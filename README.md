# JunoHub

JunoHub 是一个集成多账户调度能力的多用户 BYOK AI 网关：

- `junohub-web/`：统一登录入口、用户控制台和管理界面，默认端口 `3003`
- `junohub-server/`：Rust 权限层、模型路由、账户引擎和兼容网关，默认端口 `9003`
- Rust 账户引擎：账户池、调度、粘性会话、故障切换和上游协议能力（原 Sub2API `v0.1.182` 的 Rust 实现，Go 容器已移除）
- PostgreSQL / Redis：账户引擎的数据库和缓存，仅在 Compose 内部网络开放，数据保存在 `data/accounts/`

## Docker 部署

需要 Docker Engine、Docker Compose v2 和 OpenSSL。推荐使用一键脚本：

```bash
chmod +x deploy.sh
./deploy.sh up
```

首次运行会生成一份权限为 `600` 的配置：

- `.env`：JunoHub JWT、加密主密钥、引导管理员，以及账户引擎 PostgreSQL / Redis 凭证

脚本会在终端显示初始管理员密码。请立即保存，并长期备份 `.env` 文件。常用命令：

```bash
./deploy.sh status
./deploy.sh logs
./deploy.sh restart
./deploy.sh update
./deploy.sh down
```

部署完成后：

- JunoHub Web：`http://服务器地址:3003`
- JunoHub OpenAI-compatible API：`http://服务器地址:3003/v1`
- JunoHub Server：`http://服务器地址:9003`

新增账户引擎供应商时，Base URL 指向 Rust 网关自身（Compose 内部地址）：

```text
http://junohub-server:9003/v1
```

首次启动时 `junohub-accounts-postgres` 会使用 `deploy/accounts-schema.sql` 初始化账户引擎表结构（已存在的数据目录不会重复初始化）。

## 数据与备份

JunoHub 业务数据保存在 Docker 命名卷 `junohub-data`。账户引擎数据保存在：

```text
data/accounts/postgres/
data/accounts/redis/
```

备份或迁移前应先停止服务：

```bash
./deploy.sh down
tar -czf junohub-backup.tar.gz .env data/accounts
```

`docker compose down` 不会删除这些数据。不要在服务运行时直接复制 PostgreSQL 数据目录，也不要使用 `docker compose down -v` 删除 JunoHub 数据卷。

## 手动配置

也可以手动创建配置：

```bash
cp .env.docker.example .env
chmod 600 .env
```

必须替换示例中的所有密码和密钥。建议使用：

```bash
openssl rand -hex 32
```

配置完成后运行：

```bash
docker compose --env-file .env up -d --build --wait
```

## 本地开发

前端开发时先启动 `junohub-server`。前端通过 `JUNO_HUB_API_URL` 指向服务端；未设置时默认使用 `http://127.0.0.1:9003`。

本地开发账户引擎需要可访问的 PostgreSQL 与 Redis，通过环境变量 `SUB2API_DATABASE_URL` 和 `SUB2API_REDIS_URL` 覆盖（默认主机名 `junohub-accounts-postgres` / `junohub-accounts-redis`）。

迁移状态和兼容边界见 `junohub-server/MIGRATION.md` 与 `junohub-server/SUB2API_RUST_PORT.md`。
