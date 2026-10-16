# JunoHub

多用户 BYOK AI 网关：统一登录入口、用户控制台、模型路由、账户池调度与用量计费。

- `junohub-server/` — Rust 后端：权限层、模型路由、账户引擎、兼容网关（默认 `9003`）
- `junohub-web/` — Next.js 前端：登录、用户控制台、管理界面（默认 `3003`）
- PostgreSQL / Redis — 账户引擎的数据面，仅在 Compose 内部网络开放

## 部署

需要 Docker Engine、Docker Compose v2 和 OpenSSL。

```bash
./deploy.sh init    # 生成 .env（随机密钥 + 引导管理员密码），会打印初始密码
./deploy.sh up      # 构建并启动全部服务
./deploy.sh status  # 查看容器、数据卷、健康状态与备份概览
```

启动后：

- Web 控制台 `http://服务器地址:3003`
- OpenAI 兼容 API `http://服务器地址:3003/v1`（`Authorization: Bearer <访问密钥>`）
- 后端直连 `http://服务器地址:9003`

首次启动时 `junohub-accounts-postgres` 会用 `deploy/accounts-schema.sql` 初始化账户引擎表结构。

### 命令一览

| 命令 | 作用 |
| --- | --- |
| `init` | 生成 `.env`（已存在则不覆盖） |
| `up` / `down` / `restart` | 启动 / 停止（保留数据）/ 重启 |
| `update` | 拉取基础镜像、重新构建、滚动启动 |
| `status` | 容器 + 数据卷 + 健康 + 备份概览 |
| `backup` | 生成一份**经过校验**的完整备份 |
| `restore <目录>` | 从备份恢复（会先自动为当前状态留一份快照） |
| `prune` | 按份数清理旧备份（默认保留 7 份） |
| `migrate` | 把旧版本留在项目目录里的数据迁移进命名卷 |
| `doctor` | 环境自检：依赖、端口、权限、磁盘、旧布局残留 |
| `logs` / `shell` / `config` | 排查用 |

## 数据存放

**所有持久化数据都在 Docker 命名卷中，不在项目目录里。**

| 卷 | 内容 | 重要程度 |
| --- | --- | --- |
| `junohub-data` | 业务库（SQLite）：用户、访问密钥、模型路由、日志、账单 | 关键 |
| `junohub-accounts-postgres` | 账户引擎账户池 | 关键 |
| `junohub-accounts-redis` | 并发槽、粘性会话、限流窗口 | 可重建 |

`.env` 是配置，也**是加密主密钥**：数据里的供应商凭据用它加密，`.env` 丢了数据就解不开。请长期单独保存。

> 早期版本把 PostgreSQL / Redis 放在项目目录 `data/accounts/` 下。若你是从那个版本升级，执行 `./deploy.sh migrate` 一次性迁移；它会保留原目录为 `data.migrated.<时间戳>`，确认无误后再自行删除。

## 备份与恢复

```bash
./deploy.sh backup                        # 备份到 ./backups/<UTC 时间戳>/
./deploy.sh restore ./backups/<时间戳>     # 恢复（交互确认，可加 --yes）
```

`backup` 是在线一致性快照，不需要停服。它**会自己验证**：

1. SQLite 通过 `VACUUM INTO` 生成快照（不复制运行中的 WAL），校验文件头、完整性与行数
2. PostgreSQL 用 `pg_dump --format=custom`
3. 把 dump 恢复到一个临时库，逐表和线上比对行数
4. 任一校验失败就删掉这份备份并报错 —— 不会留下一个"看起来像备份"的空壳

备份目录包含 `deployment.env`、`junohub.sqlite`、`accounts.dump`、`manifest.json`（含各产物 sha256 与镜像版本）和 `COMPLETE` 标记。目录权限 `700`，文件 `600`。

`restore` 会先为当前状态自动生成一份 `pre-restore-<时间戳>` 快照，再替换业务库、重建账户池、清空 Redis（只含可重建的调度状态）。全程可用 `--yes` 跳过交互确认。

**不要**在服务运行时直接复制数据库目录，也不要使用 `docker compose down -v`。

### 异地备份

`./backups/` 和 Docker 卷都在同一台机器上，机器坏了就都没了。请把备份目录同步到别处：

```bash
rsync -a --delete ./backups/ user@backup-host:/srv/junohub-backups/
```

## 监控（可选）

```bash
docker compose -f compose.yaml -f compose.monitoring.yaml --profile monitoring up -d
```

Prometheus 只监听 `127.0.0.1:${JUNOHUB_MONITOR_PORT:-9090}`。`/metrics` 默认仅允许管理员 JWT，可另配 `JUNOHUB_METRICS_TOKEN`。
