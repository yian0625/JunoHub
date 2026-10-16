import assert from "node:assert/strict"
import { test } from "node:test"
import { APIError, accountAPI, authAPI } from "./api.ts"

async function withClient(run) {
  const originals = Object.fromEntries(["fetch", "window", "localStorage"].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]))
  const storage = new Map([["hub_token", "existing-session"], ["hub_user", "cached-profile"]])
  const redirects = []
  Object.defineProperty(globalThis, "window", { configurable: true, value: { location: { replace: path => redirects.push(path) } } })
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value), removeItem: key => storage.delete(key) } })
  try { await run({ storage, redirects }) }
  finally { for (const [key, descriptor] of Object.entries(originals)) { if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key] } }
}

const jsonResponse = (status, message, data = null, details = {}) => new Response(JSON.stringify({ code: status, message, data, ...details }), { status, headers: { "Content-Type": "application/json" } })

test("login 401 reports a useful error without navigating or sending a stale token", async () => withClient(async ({ storage, redirects }) => {
  globalThis.fetch = async (path, options) => {
    assert.equal(path, "/auth/login")
    assert.equal(options.headers.Authorization, undefined)
    return jsonResponse(401, "密码不正确，请重新输入。", null, { reason: "password_incorrect", field: "password" })
  }
  await assert.rejects(authAPI.login({ email: "member@example.test", password: "wrong" }), error => error instanceof APIError && error.message === "密码不正确，请重新输入。" && error.field === "password" && error.reason === "password_incorrect")
  assert.deepEqual(redirects, [])
  assert.equal(storage.get("hub_token"), "existing-session")
}))

test("authenticated 401 still clears an expired session and returns to login", async () => withClient(async ({ storage, redirects }) => {
  globalThis.fetch = async (_, options) => {
    assert.equal(options.headers.Authorization, "Bearer existing-session")
    return jsonResponse(401, "未授权")
  }
  await assert.rejects(accountAPI.profile(), /未授权/)
  assert.deepEqual(redirects, ["/login"])
  assert.equal(storage.has("hub_token"), false)
  assert.equal(storage.has("hub_user"), false)
}))

test("registration and service failures keep the server's actionable message", async () => withClient(async ({ redirects }) => {
  globalThis.fetch = async () => jsonResponse(400, "此邮箱已注册")
  await assert.rejects(authAPI.register({ email: "member@example.test", password: "fixture-password", verification_code: "123456" }), /此邮箱已注册/)
  globalThis.fetch = async () => jsonResponse(503, "登录服务暂时不可用，请稍后重试。")
  await assert.rejects(authAPI.login({ email: "member@example.test", password: "wrong" }), /登录服务暂时不可用/)
  assert.deepEqual(redirects, [])
}))

test("registration sends verification codes without a session and preserves specific code failures", async () => withClient(async ({ redirects, storage }) => {
  globalThis.fetch = async (path, options) => {
    assert.equal(path, "/auth/register/code")
    assert.equal(options.headers.Authorization, undefined)
    assert.deepEqual(JSON.parse(options.body), { email: "member@example.test" })
    return jsonResponse(200, "", { retry_after: 60, expires_in: 300 })
  }
  assert.deepEqual(await authAPI.sendRegistrationCode("member@example.test"), { retry_after: 60, expires_in: 300 })
  globalThis.fetch = async (path, options) => {
    assert.equal(path, "/auth/register")
    assert.equal(options.headers.Authorization, undefined)
    assert.equal(JSON.parse(options.body).verification_code, "123456")
    return jsonResponse(400, "验证码已过期，请重新获取。", null, { reason: "code_expired", field: "verification_code" })
  }
  await assert.rejects(authAPI.register({ email: "member@example.test", password: "fixture-password", verification_code: "123456" }), error => error instanceof APIError && error.field === "verification_code" && error.reason === "code_expired")
  globalThis.fetch = async () => jsonResponse(503, "验证码邮件服务尚未配置，请联系管理员。")
  await assert.rejects(authAPI.sendRegistrationCode("member@example.test"), /邮件服务尚未配置/)
  globalThis.fetch = async () => jsonResponse(429, "发送过于频繁，请在 60 秒后重试。")
  await assert.rejects(authAPI.sendRegistrationCode("member@example.test"), error => error instanceof APIError && error.status === 429)
  assert.deepEqual(redirects, [])
  assert.equal(storage.get("hub_token"), "existing-session")
}))

test("network failures explain retry and successful login still returns the account", async () => withClient(async ({ redirects }) => {
  globalThis.fetch = async () => { throw new TypeError("Failed to fetch") }
  await assert.rejects(authAPI.login({ email: "member@example.test", password: "wrong" }), /无法连接服务器，请检查网络后重试/)
  const account = { uid: 5, nickname: "示例用户", role: "user", token: "fixture-token" }
  globalThis.fetch = async () => jsonResponse(200, "", account)
  assert.deepEqual(await authAPI.login({ email: "member@example.test", password: "fixture-password" }), account)
  assert.deepEqual(redirects, [])
}))

 test("login errors retain precise fields and account status from the server", async () => withClient(async ({ redirects }) => {
  for (const [status, message, reason, field] of [
    [401, "该邮箱尚未注册，请先注册。", "email_not_registered", "email"],
    [401, "密码不正确，请重新输入。", "password_incorrect", "password"],
    [403, "账户已停用，请联系管理员。", "account_disabled", undefined],
  ]) {
    globalThis.fetch = async () => jsonResponse(status, message, null, { reason, field })
    await assert.rejects(authAPI.login({ email: "member@example.test", password: "fixture-password" }), error => error instanceof APIError && error.status === status && error.message === message && error.reason === reason && error.field === field)
  }
  assert.deepEqual(redirects, [])
}))
