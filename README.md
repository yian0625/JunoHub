# JunoHub

JunoHub 是独立于 Juno 核心业务服务的多用户 BYOK AI 网关。平台不提供公共模型；用户配置自己的 OpenAI-compatible 上游、模型别名和虚拟 Key：

- `junohub-web/`：统一登录入口、用户控制台与权限管理界面，默认端口 `3003`
- `junohub-server/`：Rust 网关和管理 API，默认端口 `9003`

## Docker 部署

需要 Docker Engine 及 Docker Compose。首次部署先创建生产配置：

推荐直接使用一键脚本，它会自动生成随机密钥和一个带 `admin` 权限标识的引导账户：

```bash
chmod +x deploy.sh
./deploy.sh up
```

首次运行会在终端显示引导账户密码，请立即保存。该账户与普通用户使用同一个登录页面。常用脚本命令：

```bash
./deploy.sh status
./deploy.sh logs
./deploy.sh restart
./deploy.sh update
./deploy.sh down
```

也可以手动使用 Docker Compose。首次部署先创建生产配置：

```bash
cp .env.docker.example .env
```

编辑 `.env`，至少替换以下值并妥善备份：

- `JUNOHUB_JWT_SECRET`：JWT 签名密钥。
- `JUNOHUB_ENCRYPTION_KEY`：上游 API Key 加密主密钥，部署后不能随意更换。
- `JUNOHUB_BOOTSTRAP_ADMIN_EMAIL`：初始权限账户邮箱。
- `JUNOHUB_BOOTSTRAP_ADMIN_PASSWORD`：初始权限账户密码。

建议每个密钥使用不少于 32 字节的随机值，例如通过 `openssl rand -hex 32` 生成。然后在本目录运行：

```bash
docker compose up -d --build
docker compose ps
```

部署完成后：

- Web 控制台：`http://服务器地址:3003`
- OpenAI-compatible API：`http://服务器地址:3003/v1`，也可以直接使用 `http://服务器地址:9003/v1`
- SQLite 数据：保存在 Docker 命名卷 `junohub-data` 中

常用维护命令：

```bash
docker compose logs -f
docker compose pull
docker compose up -d --build
docker compose down
```

`docker compose down` 不会删除数据库卷。只有显式执行 `docker compose down -v` 才会删除数据，请谨慎使用。

## 本地开发

本地开发先启动 `junohub-server`，再启动 `junohub-web`。前端通过 `JUNO_HUB_API_URL` 指向服务端；未设置时默认使用 `http://127.0.0.1:9003`。

迁移状态、兼容边界和切换流程见 `junohub-server/MIGRATION.md`。
