# JunoHub Server

独立 Rust 多用户 BYOK（Bring Your Own Key）AI 网关。JunoHub 不提供平台公共模型：每位用户配置自己的 OpenAI-compatible 上游地址、模型映射和虚拟 API Key，并通过统一的 `/v1` 地址访问。

## 产品边界

- 用户：注册/登录，管理自己的供应商、模型别名、虚拟 Key 和请求日志。
- 权限：用户表的 `role` 是权限标识；`admin` 可管理用户，所有账户使用同一个登录接口。
- 网关：根据虚拟 Key 确定用户，只查询该用户的模型和供应商。
- 密钥：用户密码使用 Argon2；上游 API Key 使用 AES-256-GCM 加密；虚拟 Key 仅保存 SHA-256 摘要。
- 独立性：不依赖 `juno-server` 的代码、进程、数据库或内部接口。

当前上游协议是 OpenAI-compatible JSON API，支持普通响应和流式响应。上游 API Key 可留空，适用于无需鉴权的 Ollama 等服务。multipart 音频、供应商原生协议转换和 LiteLLM 长尾协议尚不在当前范围内。

## 本地运行

需要 Rust 1.88+：

```bash
cp .env.example .env
set -a
source .env
set +a
cargo run
```

再启动前端：

```bash
cd ../junohub-web
npm run dev
```

- 用户入口：`http://localhost:3003/login`
- 用户控制台：`http://localhost:3003/console`
- 带 `admin` 权限的账户登录后自动进入用户管理页面

生产环境必须设置随机且稳定的 `JUNOHUB_JWT_SECRET`、`JUNOHUB_ENCRYPTION_KEY` 和强引导账户密码。修改加密密钥后，已有上游 API Key 将无法解密。

完整的前后端 Docker 部署请在项目根目录使用 `compose.yaml`，详见 `../README.md`。本目录的 `Dockerfile` 仅用于构建 Rust 服务镜像。

## 调用示例

用户在控制台创建虚拟 Key 后：

```bash
curl http://localhost:9003/v1/models \
  -H 'Authorization: Bearer sk-juno-...'

curl http://localhost:9003/v1/chat/completions \
  -H 'Authorization: Bearer sk-juno-...' \
  -H 'Content-Type: application/json' \
  -d '{"model":"my-model","messages":[{"role":"user","content":"hello"}]}'
```

`model` 使用用户配置的对外别名；转发时会替换为对应的上游模型名。

## 数据升级

服务默认使用自己的 SQLite 数据库 `junohub.db`。迁移 `0002_user_byok.sql` 创建用户 BYOK 数据结构。早期试验版的平台部署表会保留在既有数据库中以避免静默丢失数据，但当前代码不读取、写入或暴露这些表。

## 验证

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```
