# JunoHub 抽离与 BYOK 重构结果

## 当前架构

JunoHub 已成为独立 Rust 服务，并采用纯用户 BYOK 模式：

1. 用户注册后配置自己的 OpenAI-compatible 上游 Base URL 和可选 API Key。
2. 用户把对外模型别名映射到上游模型名。
3. 用户创建 `sk-juno-...` 虚拟 Key。
4. 网关从虚拟 Key 得到用户身份，只能解析该用户的数据并转发。
5. `admin` 只是用户记录上的权限标识，通过统一登录入口鉴权；它只允许用户管理，不拥有平台公共模型。

## 与 Juno 的边界

- `JunoHub/junohub-server` 不导入、调用或连接 `juno-server`。
- JunoHub 使用独立端口、配置、JWT、加密密钥和 SQLite 数据库。
- `juno-server` 不再包含 JunoHub 路由、服务、DAO、迁移或启动目标。
- Juno App 与管理后台不负责 JunoHub 的供应商或模型配置。
- 两套系统可独立启动、升级和停止。

## 安全与隔离

- 所有供应商、模型、虚拟 Key、日志查询均包含 `user_id` 所有权条件。
- 虚拟 Key 鉴权同时检查用户状态；带 `admin` 权限的账户停用用户后，已签发 JWT 和虚拟 Key 都会失效。
- 上游 API Key 使用 AES-256-GCM 加密且管理接口不回传明文。
- 虚拟 Key 只在创建时返回一次，数据库只保存不可逆摘要。
- 自动化测试覆盖跨用户模型/供应商不可见、停用用户网关失效和密钥加密落库。

## 兼容范围

当前网关支持 OpenAI-compatible JSON 请求、`/v1/models`、普通响应和 SSE/流式响应。它不是 LiteLLM 全量复刻；原生 Anthropic/Gemini 请求转换、multipart 音频、Responses 本地资源、MCP/A2A、SSO/SCIM 等长尾能力需要按实际需求继续实现。

早期 Rust 试验版的平台部署代码和管理入口已删除。旧迁移创建的遗留表仅为安全升级保留，运行时不再使用。
