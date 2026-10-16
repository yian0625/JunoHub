# Resend 邮箱验证码注册

如果不使用自有域名，可以改用 [SMTP 邮箱发信](smtp-registration.md)。使用 Resend 时设置 `JUNOHUB_EMAIL_PROVIDER=resend`；使用 SMTP 时设置为 `smtp`。

## 启用真实邮件

1. 在 [Resend Domains](https://resend.com/domains) 添加你持有、且 Resend 接受的域名。
2. 在该域名的 DNS 管理平台添加 Resend 提供的 DKIM、SPF 和 MX 发信验证记录。主机名、内容、优先级与区域均以你的 Resend 控制台为准；不要替换现有收信 MX。

   当前域名注册管理位于 DigitalPlat，但名称服务器为 `hal.ns.cloudflare.com` 与 `magali.ns.cloudflare.com`，DNS 记录应在 Cloudflare 添加。保留现有名称服务器。
3. 回到 Resend 完成验证，确认域名状态为 `Verified`。
4. 创建具有该域名发送权限的 API Key，在项目根目录 `.env` 中填写：

```dotenv
RESEND_API_KEY=re_你的密钥
RESEND_FROM="JunoHub <noreply@你的已验证域名>"
JUNOHUB_EMAIL_DAILY_LIMIT=100
```

API Key 仅保存在后端环境中，不提交 Git，不添加 `NEXT_PUBLIC_` 前缀。使用 Compose 部署时执行 `docker compose up -d --build junohub-server junohub-web`；仅调整 Key 或发件地址后执行 `docker compose up -d --no-deps --force-recreate junohub-server`。

## 流程与限制

- `POST /auth/register/code`，请求 `{"email":"name@example.com"}`，发送成功返回 `data.retry_after=60` 和 `data.expires_in=300`。未配置邮件服务时返回 503；不发送、不返回验证码。
- `POST /auth/register` 额外要求 `verification_code`，验证码与邮箱绑定。成功验证、创建普通用户和消耗验证码在同一事务中完成。
- 6 位随机数字，5 分钟有效；输错最多 5 次，重新发送后旧验证码失效。数据库只保存使用服务端密钥计算的 HMAC，不保存明文验证码。
- 每个邮箱 60 秒可重发一次，24 小时最多 10 次；全站每分钟最多 5 次，24 小时上限由 `JUNOHUB_EMAIL_DAILY_LIMIT` 决定，默认 100。失败的发送尝试也计入频率限制，防止反复请求消耗邮件额度。
- Resend 的实际账号额度、发送速率与项目内部限制分别执行。注册错误返回明确的 `reason` 与 `field`，邮箱或验证码错误在对应字段显示。
- 现有账户登录、引导管理员及管理员管理用户流程保持原有行为。

## 验证状态

2026-10-10，在 Resend 控制台提交一个免费公共域名被拒绝，提示：`We don't allow free public domains. Please use a domain you own instead.` 域名未创建，尚未生成验证记录；未修改 Cloudflare DNS。Resend 需要自有域名，配置后把发件地址写入 `RESEND_FROM`。代码不再内置任何默认发件域名：未配置 `RESEND_FROM` 时发送会被明确拒绝并提示缺少该变量，`RESEND_API_KEY` 未配置时同样拒绝，真实发信尚未启用。

本地测试使用内存数据库和本机模拟 Resend 服务，不向真实邮箱发信。实际收信仍需完成 API Key 和 DNS 配置后验证。

参考：[Resend 发信 API](https://resend.com/docs/api-reference/emails/send-email)、[域名验证说明](https://resend.com/docs/dashboard/domains/introduction)。
