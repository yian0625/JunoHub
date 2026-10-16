#!/usr/bin/env bash
#
# JunoHub 部署与运维脚本
#
# 所有持久化数据都在 Docker 命名卷里，项目目录只保留代码与配置。
# 备份/恢复请用本脚本的 backup / restore，不要手工复制数据目录：
# 运行中的 SQLite / PostgreSQL 数据目录直接复制是不一致的。

set -Eeuo pipefail

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
ENV_FILE="$ROOT_DIR/.env"
ENV_TEMPLATE="$ROOT_DIR/.env.docker.example"
COMPOSE_FILE="$ROOT_DIR/compose.yaml"
MONITORING_FILE="$ROOT_DIR/compose.monitoring.yaml"
BACKUP_DIR="${JUNOHUB_BACKUP_DIR:-$ROOT_DIR/backups}"
BACKUP_KEEP="${JUNOHUB_BACKUP_KEEP:-7}"
BACKUP_FORMAT=1

VOLUME_DATA=junohub-data
VOLUME_PG=junohub-accounts-postgres
VOLUME_REDIS=junohub-accounts-redis

# 容器内的运行身份，恢复数据后必须还原属主，否则服务起不来。
SERVER_UID_GID=10001:10001   # junohub（见 junohub-server/Dockerfile）
PG_UID_GID=70:70             # postgres:18-alpine 的 postgres 用户

cd "$ROOT_DIR"

# ---------------------------------------------------------------- 输出与工具

if [[ -t 1 ]]; then
  C_RESET=$'\033[0m'; C_DIM=$'\033[2m'; C_BOLD=$'\033[1m'
  C_RED=$'\033[31m'; C_GREEN=$'\033[32m'; C_YELLOW=$'\033[33m'; C_BLUE=$'\033[34m'
else
  C_RESET=""; C_DIM=""; C_BOLD=""; C_RED=""; C_GREEN=""; C_YELLOW=""; C_BLUE=""
fi

log()  { printf '%s\n' "${C_BLUE}==>${C_RESET} $*"; }
ok()   { printf '%s\n' "${C_GREEN}  ok${C_RESET} $*"; }
warn() { printf '%s\n' "${C_YELLOW}  !!${C_RESET} $*" >&2; }
die()  { printf '%s\n' "${C_RED}错误：${C_RESET}$*" >&2; exit 1; }
step() { printf '\n%s\n' "${C_BOLD}$*${C_RESET}"; }

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "缺少命令 $1${2:+（$2）}"
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

size_of() { wc -c < "$1" | tr -d ' '; }

human_size() {
  local bytes="${1:-0}"
  [[ "$bytes" =~ ^[0-9]+$ ]] || { printf '%s' "$bytes"; return; }
  awk -v b="$bytes" 'BEGIN {
    split("B KB MB GB TB PB", u, " ")
    i = 1
    while (b >= 1024 && i < 6) { b /= 1024; i++ }
    printf (i == 1 ? "%.0f%s" : "%.1f%s"), b, u[i]
  }'
}

# 从 .env 读取单个键，不 source：.env 里可能有空格或引号。
env_value() {
  local key="$1" fallback="${2:-}"
  [[ -f "$ENV_FILE" ]] || { printf '%s' "$fallback"; return; }
  local line
  line="$(grep -E "^[[:space:]]*${key}=" "$ENV_FILE" | tail -1 || true)"
  [[ -n "$line" ]] || { printf '%s' "$fallback"; return; }
  local value="${line#*=}"
  value="${value%\"}"; value="${value#\"}"
  value="${value%\'}"; value="${value#\'}"
  printf '%s' "$value"
}

random_hex() { openssl rand -hex "$1"; }

timestamp_utc() { date -u +%Y%m%dT%H%M%SZ; }

# ---------------------------------------------------------------- 前置条件

check_docker() {
  require_command docker "https://docs.docker.com/engine/install/"
  docker compose version >/dev/null 2>&1 || die "需要 Docker Compose v2（docker compose，不是 docker-compose）"
  docker info >/dev/null 2>&1 || die "Docker daemon 未运行，或当前用户无权访问"
}

check_openssl() { require_command openssl; }

compose() { docker compose --env-file "$ENV_FILE" -f "$COMPOSE_FILE" "$@"; }

require_env() {
  [[ -f "$ENV_FILE" ]] || die "缺少 ${ENV_FILE}，请先运行：./deploy.sh init"
  local mode
  mode="$(stat -f '%Lp' "$ENV_FILE" 2>/dev/null || stat -c '%a' "$ENV_FILE" 2>/dev/null || echo '')"
  if [[ -n "$mode" && "$mode" != "600" ]]; then
    warn "$ENV_FILE 权限是 ${mode}，建议 chmod 600（内含加密主密钥，泄露等于数据全解）"
  fi
}

# 等待服务健康；失败时打印相关日志尾部，避免只看到一句 "unhealthy"。
compose_up_wait() {
  if ! compose up -d --wait "$@"; then
    printf '\n' >&2
    warn "服务未能全部就绪，下面是最近的日志："
    compose ps >&2 || true
    compose logs --tail=40 --no-log-prefix >&2 || true
    die "启动失败。修复后重试 ./deploy.sh up"
  fi
}

# ---------------------------------------------------------------- 数据卷

# 在数据卷里执行命令（以 root，避免属主不匹配导致写不进去）
volume_run() {
  local volume="$1"; shift
  require_command docker
  docker run --rm --user root -v "${volume}:/vol" alpine:3 "$@"
}

volume_size_bytes() {
  docker run --rm --user root -v "${1}:/vol" alpine:3 sh -c 'du -sb /vol 2>/dev/null | cut -f1' 2>/dev/null || echo 0
}

volume_exists() { docker volume inspect "$1" >/dev/null 2>&1; }

volume_ensure() {
  if ! volume_exists "$1"; then
    docker volume create "$1" >/dev/null
    ok "已创建数据卷 $1"
  fi
}

# 命名卷的宿主机路径；让用户能确认数据到底在哪。
volume_mountpoint() {
  docker volume inspect "$1" --format '{{.Mountpoint}}' 2>/dev/null || echo '(不存在)'
}

# ---------------------------------------------------------------- init

init_env() {
  if [[ -f "$ENV_FILE" ]]; then
    ok ".env 已存在，未覆盖：$ENV_FILE"
    return
  fi
  check_openssl
  [[ -f "$ENV_TEMPLATE" ]] || die "缺少模板 $ENV_TEMPLATE"

  local jwt_secret encryption_key admin_password postgres_password redis_password
  jwt_secret="$(random_hex 32)"
  encryption_key="$(random_hex 32)"
  admin_password="$(random_hex 16)"
  postgres_password="$(random_hex 32)"
  redis_password="$(random_hex 32)"

  umask 077
  cat >"$ENV_FILE" <<EOF
JUNOHUB_JWT_SECRET=$jwt_secret
JUNOHUB_ENCRYPTION_KEY=$encryption_key
JUNOHUB_BOOTSTRAP_ADMIN_EMAIL=admin@junohub.local
JUNOHUB_BOOTSTRAP_ADMIN_PASSWORD=$admin_password
JUNOHUB_WEB_PORT=3003
JUNOHUB_API_PORT=9003
RUST_LOG=junohub_server=info,tower_http=info

# 日志明细保留天数（0 = 永久保留）。只清理逐条明细，
# 密钥上的累计请求数、token 与费用不受影响。
JUNOHUB_LOG_RETENTION_DAYS=90

# 账户引擎并发槽 TTL（分钟），决定异常退出后槽位最长滞留时间。
GATEWAY_CONCURRENCY_SLOT_TTL_MINUTES=3

# 账户引擎数据面（PostgreSQL + Redis）
POSTGRES_USER=sub2api
POSTGRES_PASSWORD=$postgres_password
POSTGRES_DB=sub2api
REDIS_PASSWORD=$redis_password
REDIS_DB=0
ADMIN_EMAIL=admin@sub2api.local

# 邮箱注册验证码（可选）：smtp 或 resend
JUNOHUB_EMAIL_PROVIDER=smtp
JUNOHUB_EMAIL_DAILY_LIMIT=100
SMTP_HOST=
SMTP_PORT=465
SMTP_SECURITY=tls
SMTP_USERNAME=
SMTP_PASSWORD=
SMTP_FROM=

# 图片存储（可选，阿里云 OSS）
OSS_BUCKET=
OSS_REGION=
OSS_ENDPOINT=
OSS_PREFIX=JunoHub
OSS_ACCESS_KEY_ID=
OSS_ACCESS_KEY_SECRET=
OSS_SECURITY_TOKEN=

# 专用 /metrics 采集凭据；留空时仅允许管理员 JWT
JUNOHUB_METRICS_TOKEN=
EOF
  chmod 600 "$ENV_FILE"

  step "已生成安全配置"
  echo "  文件            ${ENV_FILE}（权限 600）"
  echo "  引导管理员      admin@junohub.local"
  echo "  初始密码        ${C_BOLD}${admin_password}${C_RESET}"
  echo
  warn "请立刻保存上面的密码，并长期备份 .env —— 它同时是数据解密密钥，丢了数据就解不开。"
}

# ---------------------------------------------------------------- 命令：生命周期

cmd_up() {
  check_docker; require_env
  volume_ensure "$VOLUME_DATA"
  volume_ensure "$VOLUME_PG"
  volume_ensure "$VOLUME_REDIS"
  log "构建并启动全部服务"
  compose_up_wait --build
  cmd_status
  local web_address
  web_address="$(compose port junohub-web 3003 2>/dev/null || true)"
  printf '\n%s\n' "${C_GREEN}JunoHub 已启动${C_RESET}：http://${web_address:-127.0.0.1:3003}"
}

cmd_down() {
  check_docker; require_env
  log "停止服务（数据卷保留）"
  compose down --remove-orphans
  ok "已停止。数据仍在命名卷中：$VOLUME_DATA / $VOLUME_PG / $VOLUME_REDIS"
  echo "  重新启动：./deploy.sh up"
}

cmd_restart() {
  check_docker; require_env
  log "重启服务"
  compose restart
  compose up -d --wait
  cmd_status
}

cmd_update() {
  check_docker; require_env
  log "更新：拉取基础镜像并重新构建"
  compose pull junohub-accounts-postgres junohub-accounts-redis
  compose build --pull
  log "滚动启动（等待健康检查通过）"
  compose_up_wait --remove-orphans
  ok "更新完成"
  warn "若本次更新包含数据库迁移，请先执行 ./deploy.sh backup 留一份回滚点。"
}

cmd_logs() {
  check_docker; require_env
  local service="${1:-}"
  if [[ -n "$service" ]]; then compose logs -f --tail=200 "$service"
  else compose logs -f --tail=200; fi
}

cmd_config() {
  check_docker; require_env
  compose config
}

cmd_shell() {
  check_docker; require_env
  local service="${1:-junohub-server}"
  local shell_bin="${2:-sh}"
  compose exec "$service" "$shell_bin"
}

cmd_client() {
  require_command cargo "https://rustup.rs"
  log "安装 JunoHub 本地连接器"
  cargo install --path "$ROOT_DIR/junohub-server" --bin junohub-connect --force
  junohub-connect install-handler
  ok "已安装：$(command -v junohub-connect)"
}

# ---------------------------------------------------------------- 命令：状态

cmd_status() {
  check_docker; require_env
  step "容器"
  compose ps

  step "数据卷"
  local vol
  for vol in "$VOLUME_DATA" "$VOLUME_PG" "$VOLUME_REDIS"; do
    if volume_exists "$vol"; then
      printf '  %-28s %10s  %s\n' "$vol" "$(human_size "$(volume_size_bytes "$vol")")" "$(volume_mountpoint "$vol")"
    else
      printf '  %-28s %10s\n' "$vol" "缺失"
    fi
  done

  step "服务健康"
  local api_port web_port
  api_port="$(env_value JUNOHUB_API_PORT 9003)"
  web_port="$(env_value JUNOHUB_WEB_PORT 3003)"
  if curl -fsS --max-time 5 "http://127.0.0.1:${api_port}/health/readiness" >/dev/null 2>&1; then
    ok "后端 readiness http://127.0.0.1:${api_port}/health/readiness"
  else
    warn "后端 readiness 不可达（端口 ${api_port}）"
  fi
  if curl -fsS --max-time 5 -o /dev/null "http://127.0.0.1:${web_port}/login" 2>/dev/null; then
    ok "前端登录页 http://127.0.0.1:${web_port}/login"
  else
    warn "前端登录页不可达（端口 ${web_port}）"
  fi

  step "备份"
  if [[ -d "$BACKUP_DIR" ]]; then
    local count
    count="$(find "$BACKUP_DIR" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
    echo "  目录   $BACKUP_DIR   共 ${count} 份（保留最近 ${BACKUP_KEEP} 份）"
    find "$BACKUP_DIR" -mindepth 1 -maxdepth 1 -type d -print 2>/dev/null | sort | tail -3 | sed 's/^/    /'
  else
    echo "  还没有备份。执行：./deploy.sh backup"
  fi
}

# ---------------------------------------------------------------- 命令：备份

cmd_backup() {
  check_docker; require_env
  create_backup "$BACKUP_DIR/$(timestamp_utc)"
  cmd_prune_quiet
}

# 生成一份完整备份到 $1。备份里含 .env、账户池与凭证，全程最小可见性。
create_backup() {
  local dest="$1"
  [[ -e "$dest" ]] && die "备份目录已存在：$dest"
  mkdir -p "$dest"
  umask 077
  chmod 700 "$dest"

  log "启动服务以获取一致性快照（在线备份不会中断请求）"
  compose_up_wait >/dev/null

  local server_container pg_container redis_container
  server_container="$(compose ps -q junohub-server)"
  pg_container="$(compose ps -q junohub-accounts-postgres)"
  redis_container="$(compose ps -q junohub-accounts-redis)"
  [[ -n "$server_container" ]] || die "找不到 junohub-server 容器"

  # --- 部署配置 -----------------------------------------------------------
  log "备份配置 (.env)"
  cp "$ENV_FILE" "$dest/deployment.env"
  chmod 600 "$dest/deployment.env"

  # --- 业务库（SQLite）---------------------------------------------------
  # VACUUM INTO 生成一致性快照，不复制正在写入的 WAL 文件。
  log "备份业务库（SQLite，用户/密钥/模型/日志/账单）"
  compose exec -T junohub-server sh -c 'rm -f /data/.deploy-backup.sqlite'
  compose exec -T junohub-server junohub-backup /data/.deploy-backup.sqlite
  docker cp "${server_container}:/data/.deploy-backup.sqlite" "$dest/junohub.sqlite"
  compose exec -T junohub-server sh -c 'rm -f /data/.deploy-backup.sqlite'

  # --- 账户引擎（PostgreSQL）--------------------------------------------
  log "备份账户引擎账户池（PostgreSQL，pg_dump 自定义格式）"
  compose exec -T junohub-accounts-postgres sh -c \
    'pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" --format=custom --no-owner --no-acl' \
    > "$dest/accounts.dump"

  # --- 调度缓存（Redis）-------------------------------------------------
  # Redis 只保存可重建的调度状态（并发槽、粘性会话、限流窗口），
  # 不是业务真相；这里留一份快照仅供排查，恢复时不会使用。
  if [[ -n "$redis_container" ]]; then
    log "备份调度缓存快照（Redis，可重建，仅供排查）"
    compose exec -T junohub-accounts-redis sh -c \
      'REDISCLI_AUTH="$REDIS_PASSWORD" redis-cli --no-auth-warning --rdb /data/.deploy-backup.rdb' >/dev/null
    docker cp "${redis_container}:/data/.deploy-backup.rdb" "$dest/redis.rdb" 2>/dev/null || warn "Redis 快照跳过"
    compose exec -T junohub-accounts-redis sh -c 'rm -f /data/.deploy-backup.rdb' || true
  fi

  # --- 清单与校验 --------------------------------------------------------
  log "写入清单与校验和"
  local commit subject
  commit="$(git -C "$ROOT_DIR" rev-parse HEAD 2>/dev/null || echo unknown)"
  subject="$(git -C "$ROOT_DIR" log -1 --pretty=%s 2>/dev/null || echo unknown)"
  local sqlite_sha pg_sha sqlite_size pg_size
  sqlite_sha="$(sha256_of "$dest/junohub.sqlite")"
  sqlite_size="$(size_of "$dest/junohub.sqlite")"
  pg_sha="$(sha256_of "$dest/accounts.dump")"
  pg_size="$(size_of "$dest/accounts.dump")"

  local images
  images="$(compose images --format json 2>/dev/null | tr -d '\n' || echo '[]')"
  [[ -n "$images" ]] || images='[]'

  cat > "$dest/manifest.json" <<EOF
{
  "format": $BACKUP_FORMAT,
  "created_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "host": "$(hostname)",
  "git_commit": "$commit",
  "git_subject": "$subject",
  "artifacts": {
    "deployment.env": { "sha256": "$(sha256_of "$dest/deployment.env")", "bytes": $(size_of "$dest/deployment.env") },
    "junohub.sqlite":  { "sha256": "$sqlite_sha", "bytes": $sqlite_size },
    "accounts.dump":   { "sha256": "$pg_sha",     "bytes": $pg_size }
  },
  "volumes": {
    "junohub-data": "$VOLUME_DATA",
    "junohub-accounts-postgres": "$VOLUME_PG",
    "junohub-accounts-redis": "$VOLUME_REDIS"
  },
  "images": $images
}
EOF

  chmod 600 "$dest"/* 2>/dev/null || true

  # COMPLETE 最后写入：只有它存在，这份备份才算完整可用。
  printf '%s\n' "backup complete $(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$dest/COMPLETE"

  # 备份必须能恢复、且内容与线上一致；否则这不是备份，是假象。
  log "校验备份可恢复且内容与线上一致"
  local sqlite_probe pg_probe
  if ! sqlite_probe="$(verify_sqlite_dump "$dest/junohub.sqlite")"; then
    rm -rf "$dest"
    die "SQLite 快照校验失败，已删除这份无效备份。请检查服务状态后重试。"
  fi
  if ! pg_probe="$(verify_accounts_dump "$dest/accounts.dump")"; then
    rm -rf "$dest"
    die "账户池备份校验失败，已删除这份无效备份。请检查服务状态后重试。"
  fi
  ok "业务库 $sqlite_probe"
  ok "账户池 $pg_probe"

  if [[ "$pg_probe" == *"accounts=0"* ]]; then
    warn "账户池里没有任何账户 —— 如果你的账户池本应非空，说明它连的是新建的空库。"
    warn "请检查是否有旧的 data/accounts/postgres 目录尚未迁移：./deploy.sh doctor"
  fi

  step "备份完成"
  echo "  目录   $dest"
  echo "  业务库 $(human_size "$sqlite_size")   sha256 ${sqlite_sha:0:16}…   $sqlite_probe"
  echo "  账户池 $(human_size "$pg_size")   sha256 ${pg_sha:0:16}…   $pg_probe"
  echo "  恢复   ./deploy.sh restore $dest"
}

# 备份目录里的数据属于隐私，备份本身也应有权限约束。
cmd_prune() { cmd_prune_quiet; }

cmd_prune_quiet() {
  [[ -d "$BACKUP_DIR" ]] || return 0
  local keep="$BACKUP_KEEP"
  [[ "$keep" =~ ^[0-9]+$ ]] || keep=7
  local all
  all="$(find "$BACKUP_DIR" -mindepth 1 -maxdepth 1 -type d -print 2>/dev/null | sort)"
  local total
  total="$(printf '%s\n' "$all" | grep -c . || true)"
  (( total > keep )) || return 0
  local victims
  victims="$(printf '%s\n' "$all" | head -n "$(( total - keep ))")"
  local dir
  while IFS= read -r dir; do
    [[ -n "$dir" ]] || continue
    warn "清理旧备份 ${dir}（保留最近 ${keep} 份，可用 JUNOHUB_BACKUP_KEEP 调整）"
    rm -rf "$dir"
  done <<< "$victims"
}

# 真正验证一份备份：把 dump 恢复到临时库，逐表与线上对比行数。
# 只看文件大小或 TOC 是不够的 —— 空 dump 也有完整的表结构和目录。
verify_accounts_dump() {
  local dump="$1"
  local container
  container="$(compose ps -q junohub-accounts-postgres)"
  [[ -n "$container" ]] || { warn "Postgres 容器未运行，跳过备份校验"; return 0; }

  local scratch="junohub_backup_verify"
  docker cp "$dump" "${container}:/tmp/junohub-verify.dump" >/dev/null 2>&1 \
    || { warn "无法把 dump 送入容器校验"; return 1; }

  local probe="select 'accounts='||(select count(*) from accounts)||' groups='||(select count(*) from groups)||' api_keys='||(select count(*) from api_keys)||' users='||(select count(*) from users)"

  local live restored
  live="$(compose exec -T junohub-accounts-postgres sh -c "psql -tA -U \"\$POSTGRES_USER\" -d \"\$POSTGRES_DB\" -c \"$probe\"" 2>/dev/null | tr -d '\r')"

  compose exec -T junohub-accounts-postgres sh -c \
    "psql -q -v ON_ERROR_STOP=1 -U \"\$POSTGRES_USER\" -d postgres -c 'DROP DATABASE IF EXISTS $scratch' -c 'CREATE DATABASE $scratch'" >/dev/null 2>&1

  if ! compose exec -T junohub-accounts-postgres sh -c \
      "pg_restore -U \"\$POSTGRES_USER\" -d $scratch --no-owner --no-acl --exit-on-error /tmp/junohub-verify.dump" >/tmp/junohub-verify.err 2>&1; then
    warn "备份无法恢复：dump 已损坏或不完整"
    sed 's/^/      /' /tmp/junohub-verify.err | head -5 >&2
    compose exec -T junohub-accounts-postgres sh -c "psql -q -U \"\$POSTGRES_USER\" -d postgres -c 'DROP DATABASE IF EXISTS $scratch'" >/dev/null 2>&1
    return 1
  fi

  restored="$(compose exec -T junohub-accounts-postgres sh -c "psql -tA -U \"\$POSTGRES_USER\" -d $scratch -c \"$probe\"" 2>/dev/null | tr -d '\r')"
  compose exec -T junohub-accounts-postgres sh -c \
    "psql -q -U \"\$POSTGRES_USER\" -d postgres -c 'DROP DATABASE IF EXISTS $scratch'" >/dev/null 2>&1
  compose exec -T junohub-accounts-postgres sh -c 'rm -f /tmp/junohub-verify.dump' >/dev/null 2>&1 || true

  if [[ "$live" != "$restored" ]]; then
    warn "备份内容与线上不一致：线上 [$live] vs 备份 [$restored]"
    return 1
  fi
  printf '%s' "$restored"
  return 0
}

# SQLite 快照：先查文件头，有 python3 时再做一次完整性与行数校验。
verify_sqlite_dump() {
  local dump="$1"
  local header
  header="$(head -c 15 "$dump" 2>/dev/null || true)"
  [[ "$header" == "SQLite format 3" ]] || { warn "SQLite 快照文件头无效"; return 1; }
  command -v python3 >/dev/null 2>&1 || return 0
  python3 - "$dump" <<'PYEOF' 2>/dev/null
import sqlite3, sys
try:
    c = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
    if c.execute("pragma integrity_check").fetchone()[0] != "ok":
        sys.exit(1)
    print("users=%d keys=%d models=%d logs=%d" % (
        c.execute("select count(*) from users").fetchone()[0],
        c.execute("select count(*) from virtual_keys").fetchone()[0],
        c.execute("select count(*) from user_models").fetchone()[0],
        c.execute("select count(*) from user_request_logs").fetchone()[0]))
except Exception:
    sys.exit(1)
PYEOF
}

# ---------------------------------------------------------------- 命令：恢复

verify_backup_dir() {
  local dir="$1"
  [[ -d "$dir" ]] || die "备份目录不存在：$dir"
  [[ -f "$dir/COMPLETE" ]] || die "备份不完整（缺少 COMPLETE 标记）：$dir"
  [[ -f "$dir/manifest.json" ]] || die "备份缺少 manifest.json：$dir"
  [[ -f "$dir/junohub.sqlite" ]] || die "备份缺少 junohub.sqlite：$dir"
  [[ -f "$dir/accounts.dump" ]] || die "备份缺少 accounts.dump：$dir"

  local name expected actual
  for name in junohub.sqlite accounts.dump; do
    expected="$(sed -n "s/.*\"${name}\"[^}]*\"sha256\": \"\([a-f0-9]*\)\".*/\1/p" "$dir/manifest.json")"
    if [[ -n "$expected" ]]; then
      actual="$(sha256_of "$dir/$name")"
      [[ "$expected" == "$actual" ]] || die "$name 校验和不匹配，备份可能已损坏"
      ok "$name 校验通过"
    fi
  done
}

cmd_restore() {
  local dir="${1:-}"
  shift || true
  local force=0
  for arg in "$@"; do [[ "$arg" == "--yes" || "$arg" == "-y" ]] && force=1; done
  [[ -n "$dir" ]] || die "用法：./deploy.sh restore <备份目录> [--yes]
先用 ./deploy.sh status 查看可用备份，或 ./deploy.sh backup 创建一份。"
  check_docker; require_env
  dir="$(CDPATH= cd -- "$dir" && pwd)"
  verify_backup_dir "$dir"

  step "即将恢复"
  echo "  来源   $dir"
  echo "  目标   覆盖当前业务库与账户池；当前数据会被替换"
  echo "  Redis  将清空（只含可重建的调度状态）"
  warn "当前 .env 会先随快照存进 $BACKUP_DIR/pre-restore-<时间戳>/deployment.env，再被覆盖。"
  if (( ! force )); then
    printf '继续请输入 yes：'
    local answer=""
    read -r answer || true
    [[ "$answer" == "yes" ]] || die "已取消"
  fi

  # 恢复会替换业务库并重建账户池。先留一份当前状态的快照，
  # 万一恢复过程中失败，数据还在。
  log "先为当前状态生成一份安全快照"
  compose up -d --wait >/dev/null 2>&1 || warn "当前服务未就绪，安全快照可能不完整"
  create_backup "$BACKUP_DIR/pre-restore-$(timestamp_utc)"

  log "停止服务"
  compose down --remove-orphans || true

  log "恢复配置 (.env)"
  # 当前 .env 已经由上面的 pre-restore 快照保存为 deployment.env。
  # 不要再往项目根目录写 .env.before-restore.*：那种文件名不在 .gitignore
  # 的 `.env` 规则覆盖范围内，会把加密主密钥暴露成可提交文件。
  [[ -f "$ENV_FILE" ]] || die "当前缺少 $ENV_FILE，无法继续"
  cp "$dir/deployment.env" "$ENV_FILE"
  chmod 600 "$ENV_FILE"

  log "恢复业务库（SQLite）"
  volume_ensure "$VOLUME_DATA"
  # 必须删掉旧的 -wal/-shm，否则 SQLite 会拿旧日志去盖新库。
  docker run --rm --user root \
    -v "${VOLUME_DATA}:/data" \
    -v "${dir}:/backup:ro" \
    alpine:3 sh -c "
      rm -f /data/junohub.db /data/junohub.db-wal /data/junohub.db-shm
      cp /backup/junohub.sqlite /data/junohub.db
      chown ${SERVER_UID_GID} /data/junohub.db
      chmod 600 /data/junohub.db"

  log "重建账户池（PostgreSQL）"
  docker volume rm "$VOLUME_PG" >/dev/null 2>&1 || true
  volume_ensure "$VOLUME_PG"
  compose up -d --wait junohub-accounts-postgres
  # 用 dump 里的 DDL 完全重建，避免与初始化脚本建出的表结构冲突。
  compose exec -T junohub-accounts-postgres sh -c \
    'psql -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d postgres -c "DROP DATABASE IF EXISTS \"$POSTGRES_DB\" WITH (FORCE)" -c "CREATE DATABASE \"$POSTGRES_DB\""'
  compose exec -T junohub-accounts-postgres sh -c \
    'pg_restore -U "$POSTGRES_USER" -d "$POSTGRES_DB" --no-owner --no-acl --exit-on-error' \
    < "$dir/accounts.dump"
  ok "账户池已恢复"

  log "清空调度缓存（Redis）"
  compose up -d --wait junohub-accounts-redis
  compose exec -T junohub-accounts-redis sh -c \
    'REDISCLI_AUTH="$REDIS_PASSWORD" redis-cli --no-auth-warning FLUSHALL' >/dev/null
  ok "Redis 已清空；并发槽与粘性会话会按需重建"

  log "启动全部服务"
  compose_up_wait
  cmd_status
  printf '\n%s\n' "${C_GREEN}恢复完成${C_RESET}"
}

# ---------------------------------------------------------------- 命令：迁移旧布局

cmd_migrate() {
  check_docker; require_env
  local old_pg="$ROOT_DIR/data/accounts/postgres"
  local old_redis="$ROOT_DIR/data/accounts/redis"
  local old_backups="$ROOT_DIR/data/backups"

  if [[ ! -d "$old_pg" && ! -d "$old_redis" ]]; then
    ok "没有发现旧的项目目录内数据（data/accounts/），无需迁移"
    return
  fi

  step "迁移：项目目录内的数据 → Docker 命名卷"
  echo "  来源   data/accounts/postgres、data/accounts/redis"
  echo "  目标   ${VOLUME_PG}、$VOLUME_REDIS"
  echo "  旧目录会改名为 data.migrated.<时间戳> 保留，不会删除。"
  printf '继续请输入 yes：'
  local answer=""
  read -r answer || true
  [[ "$answer" == "yes" ]] || die "已取消"

  log "停止服务"
  compose down --remove-orphans || true

  if [[ -d "$old_pg" ]]; then
    # 目标卷若已被初始化过，直接覆盖会得到一个新旧混合的数据目录。
    # 迁移时以旧目录为唯一真相，所以先彻底删掉目标卷再复制。
    if volume_exists "$VOLUME_PG"; then
      warn "删除已存在的目标卷 $VOLUME_PG（以旧目录为准重建）"
      docker volume rm "$VOLUME_PG" >/dev/null 2>&1 || true
    fi
    volume_ensure "$VOLUME_PG"
    log "复制 PostgreSQL 数据（需要一点时间）"
    # -a 保留原属性；随后统一 chown 成容器内 postgres 用户，
    # 因为 macOS 的 VirtioFS 会把属主显示成 0:0，直接拷过去 PostgreSQL 会拒绝启动。
    docker run --rm --user root \
      -v "${VOLUME_PG}:/to" \
      -v "${old_pg}:/from:ro" \
      alpine:3 sh -c "cp -a /from/. /to/ && chown -R ${PG_UID_GID} /to && chmod 700 /to"
    ok "PostgreSQL 数据已迁移"
  fi

  if [[ -d "$old_redis" ]]; then
    if volume_exists "$VOLUME_REDIS"; then
      docker volume rm "$VOLUME_REDIS" >/dev/null 2>&1 || true
    fi
    volume_ensure "$VOLUME_REDIS"
    log "复制 Redis 数据（可重建，但保留以免丢粘性会话）"
    docker run --rm --user root \
      -v "${VOLUME_REDIS}:/to" \
      -v "${old_redis}:/from:ro" \
      alpine:3 sh -c 'cp -a /from/. /to/ && chown -R 0:0 /to'
    ok "Redis 数据已迁移"
  fi

  if [[ -d "$old_backups" ]]; then
    log "移动历史备份到 $BACKUP_DIR"
    mkdir -p "$BACKUP_DIR"
    local item
    for item in "$old_backups"/*; do
      [[ -e "$item" ]] || continue
      mv "$item" "$BACKUP_DIR/"
    done
    ok "历史备份已移出"
  fi

  local stamp
  stamp="$(timestamp_utc)"
  mv "$ROOT_DIR/data" "$ROOT_DIR/data.migrated.${stamp}"
  ok "旧目录已保留为 data.migrated.${stamp}（确认无误后可自行删除）"

  log "启动服务并验证"
  compose_up_wait
  cmd_status

  step "迁移完成"
  local accounts_count
  accounts_count="$(compose exec -T junohub-accounts-postgres sh -c \
    'psql -tA -U "$POSTGRES_USER" -d "$POSTGRES_DB" -c "select count(*) from accounts"' 2>/dev/null | tr -d '[:space:]')"
  echo "  账户池账户数   ${accounts_count:-未知}"
  if [[ "$accounts_count" == "0" && -d "$ROOT_DIR/data.migrated."* ]]; then
    warn "新库账户数为 0，但旧目录仍存在 —— 迁移可能未生效，请不要删除 data.migrated.*，先排查。"
  fi
  echo "  现在数据都在命名卷里，项目目录不再包含数据库。"
  echo "  请立刻做一份备份：./deploy.sh backup"
}

# ---------------------------------------------------------------- 命令：自检

cmd_doctor() {
  step "环境自检"
  local problems=0 notes=0

  if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
    ok "Docker $(docker version --format '{{.Server.Version}}' 2>/dev/null)"
  else
    warn "Docker 不可用"; problems=$((problems+1))
  fi
  if docker compose version >/dev/null 2>&1; then
    ok "Docker Compose $(docker compose version --short 2>/dev/null)"
  else
    warn "Docker Compose v2 不可用"; problems=$((problems+1))
  fi
  command -v openssl >/dev/null 2>&1 && ok "openssl" || { warn "缺少 openssl"; problems=$((problems+1)); }

  if [[ -f "$ENV_FILE" ]]; then
    ok ".env 存在"
    local mode
    mode="$(stat -f '%Lp' "$ENV_FILE" 2>/dev/null || stat -c '%a' "$ENV_FILE" 2>/dev/null || echo '')"
    [[ "$mode" == "600" ]] && ok ".env 权限 600" || { warn ".env 权限是 ${mode}，建议 chmod 600"; problems=$((problems+1)); }
    [[ "$(env_value JUNOHUB_ENCRYPTION_KEY)" == replace-* ]] && { warn ".env 里还是模板占位值，必须替换"; problems=$((problems+1)); }
  else
    warn "缺少 .env（运行 ./deploy.sh init 生成）"; problems=$((problems+1))
  fi

  local api_port web_port
  api_port="$(env_value JUNOHUB_API_PORT 9003)"
  web_port="$(env_value JUNOHUB_WEB_PORT 3003)"
  local p
  for p in "$api_port" "$web_port"; do
    if docker ps --format '{{.Ports}}' | grep -q ":${p}->"; then
      ok "端口 ${p} 已被本项目占用"
    elif command -v lsof >/dev/null 2>&1 && lsof -nP -iTCP:"$p" -sTCP:LISTEN >/dev/null 2>&1; then
      warn "端口 ${p} 被其他程序占用"; problems=$((problems+1))
    else
      ok "端口 ${p} 空闲"
    fi
  done

  local avail_kb
  avail_kb="$(df -Pk "$ROOT_DIR" | awk 'NR==2{print $4}')"
  if [[ -n "$avail_kb" ]] && (( avail_kb < 5*1024*1024 )); then
    warn "可用磁盘不足 5GB（当前 $(( avail_kb/1024 ))MB），构建镜像可能失败"; problems=$((problems+1))
  else
    ok "磁盘空间充足（$(( avail_kb/1024/1024 ))GB 可用）"
  fi

  # 旧布局残留
  if [[ -d "$ROOT_DIR/data/accounts" ]]; then
    warn "发现旧的项目目录内数据 data/accounts —— 建议执行 ./deploy.sh migrate"; problems=$((problems+1))
    if volume_exists "$VOLUME_PG"; then
      warn "同时目标卷 $VOLUME_PG 已存在：此时若直接启动，Postgres 会用新卷跑一个空库。"
      warn "务必先执行 ./deploy.sh migrate，不要先执行 ./deploy.sh up。"
    fi
  fi
  if compgen -G "$ROOT_DIR/data.migrated.*" >/dev/null 2>&1; then
    warn "存在 data.migrated.* 旧数据目录，确认新布局无误后可删除"
    notes=$((notes+1))
  fi

  # 遗留的自动命名卷（历史上 compose 没有显式 name 时产生）
  if docker volume inspect junohub-junohub-data >/dev/null 2>&1; then
    warn "发现遗留卷 junohub-junohub-data（来自旧版本 compose），确认无用后可：docker volume rm junohub-junohub-data"
    notes=$((notes+1))
  fi

  step "自检结束"
  if (( problems > 0 )); then warn "有 ${problems} 处问题需要处理"; fi
  if (( notes > 0 )); then warn "另有 ${notes} 条提示（不影响运行）"; fi
  if (( problems == 0 && notes == 0 )); then ok "未发现问题"; fi
  return 0
}

# ---------------------------------------------------------------- 帮助

usage() {
  cat <<'EOF'
JunoHub 部署与运维脚本

使用：./deploy.sh <命令> [参数]

生命周期
  init                生成 .env（随机密钥与引导管理员密码）
  up                  构建并启动全部服务（默认）
  down                停止服务，数据卷保留
  restart             重启全部服务
  update              拉取基础镜像、重新构建、滚动启动
  client              安装 JunoHub 本地连接器（junohub-connect）

数据
  status              容器、数据卷、健康状态、备份概览
  backup              生成一份完整备份（在线一致快照）
  restore <目录> [--yes]
                      从备份恢复（覆盖业务库与账户池，清空 Redis）
  prune               按保留份数清理旧备份（默认保留 7 份）
  migrate             把旧的项目目录内数据迁移进命名卷（一次性）

排查
  logs [服务]         持续查看日志（可指定 junohub-server / junohub-web 等）
  shell [服务] [shell]  进入容器（默认 junohub-server / sh）
  doctor              环境自检：依赖、端口、权限、磁盘、旧布局残留
  config              输出最终的 Compose 配置

  help                显示本帮助

环境变量
  JUNOHUB_BACKUP_DIR  备份目录（默认 ./backups）
  JUNOHUB_BACKUP_KEEP 保留备份份数（默认 7）

数据存放位置
  所有持久化数据都在 Docker 命名卷中，不在项目目录：
    junohub-data               业务库（用户、访问密钥、模型路由、日志、账单）
    junohub-accounts-postgres  账户引擎账户池
    junohub-accounts-redis     调度状态（可重建）
  不要直接复制运行中的数据库目录；请使用 backup / restore。
EOF
}

# ---------------------------------------------------------------- 入口

command_name="${1:-up}"
shift || true

case "$command_name" in
  init)     init_env ;;
  up)       cmd_up ;;
  down)     cmd_down ;;
  restart)  cmd_restart ;;
  update)   cmd_update ;;
  logs)     cmd_logs "${1:-}" ;;
  status|ps) cmd_status ;;
  config)   cmd_config ;;
  shell)    cmd_shell "${1:-}" "${2:-}" ;;
  client)   cmd_client ;;
  backup)   cmd_backup ;;
  restore)  cmd_restore "$@" ;;
  prune)    cmd_prune ;;
  migrate)  cmd_migrate ;;
  doctor)   cmd_doctor ;;
  help|-h|--help) usage ;;
  *)
    printf '%s\n' "${C_RED}错误：${C_RESET}未知命令 $command_name" >&2
    printf '\n' >&2
    usage >&2
    exit 2
    ;;
esac
