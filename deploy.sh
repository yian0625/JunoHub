#!/usr/bin/env bash

set -Eeuo pipefail

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
ENV_FILE="$ROOT_DIR/.env"

cd "$ROOT_DIR"

usage() {
  cat <<'EOF'
用法：./deploy.sh <命令>

命令：
  init      生成 .env、随机密钥和引导账户密码
  up        构建并启动全部服务（默认）
  down      停止并删除容器，保留数据库卷
  restart   重启全部服务
  update    拉取基础镜像、重新构建并滚动启动
  logs      持续查看全部服务日志
  status    查看容器与健康状态
  config    检查并输出最终 Compose 配置
  help      显示帮助
EOF
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "错误：缺少命令 $1" >&2
    exit 1
  fi
}

check_docker() {
  require_command docker
  if ! docker compose version >/dev/null 2>&1; then
    echo "错误：需要 Docker Compose v2" >&2
    exit 1
  fi
  if ! docker info >/dev/null 2>&1; then
    echo "错误：Docker daemon 未运行或当前用户无权访问" >&2
    exit 1
  fi
}

random_hex() {
  openssl rand -hex "$1"
}

init_env() {
  if [[ -f "$ENV_FILE" ]]; then
    echo ".env 已存在，未覆盖：$ENV_FILE"
    return
  fi
  require_command openssl
  local jwt_secret encryption_key admin_password
  jwt_secret="$(random_hex 32)"
  encryption_key="$(random_hex 32)"
  admin_password="$(random_hex 16)"
  umask 077
  cat >"$ENV_FILE" <<EOF
JUNOHUB_JWT_SECRET=$jwt_secret
JUNOHUB_ENCRYPTION_KEY=$encryption_key
JUNOHUB_BOOTSTRAP_ADMIN_EMAIL=admin@junohub.local
JUNOHUB_BOOTSTRAP_ADMIN_PASSWORD=$admin_password
JUNOHUB_WEB_PORT=3003
JUNOHUB_API_PORT=9003
RUST_LOG=junohub_server=info,tower_http=info
EOF
  chmod 600 "$ENV_FILE"
  echo "已生成安全配置：$ENV_FILE"
  echo "引导账户：admin@junohub.local（role=admin）"
  echo "账户密码：$admin_password"
  echo "该账户与普通用户使用同一登录入口；请立即保存密码，并长期备份 JUNOHUB_ENCRYPTION_KEY。"
}

ensure_env() {
  if [[ ! -f "$ENV_FILE" ]]; then
    echo "未发现 .env，正在生成安全配置。"
    init_env
  fi
}

compose() {
  docker compose --env-file "$ENV_FILE" "$@"
}

command_name="${1:-up}"

case "$command_name" in
  init)
    init_env
    ;;
  up)
    check_docker
    ensure_env
    compose up -d --build --wait
    compose ps
    web_address="$(compose port junohub-web 3003 2>/dev/null || true)"
    echo "JunoHub 已启动：http://${web_address:-127.0.0.1:3003}"
    ;;
  down)
    check_docker
    ensure_env
    compose down
    echo "JunoHub 已停止，数据库卷已保留。"
    ;;
  restart)
    check_docker
    ensure_env
    compose restart
    compose ps
    ;;
  update)
    check_docker
    ensure_env
    compose build --pull
    compose up -d --remove-orphans --wait
    compose ps
    ;;
  logs)
    check_docker
    ensure_env
    compose logs -f --tail=200
    ;;
  status)
    check_docker
    ensure_env
    compose ps
    ;;
  config)
    check_docker
    ensure_env
    compose config
    ;;
  help|-h|--help)
    usage
    ;;
  *)
    echo "错误：未知命令 $command_name" >&2
    usage >&2
    exit 2
    ;;
esac
