import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { test } from "node:test"
import ts from "typescript"

const output = ts.transpileModule(readFileSync(new URL("./model-api.ts", import.meta.url), "utf8"), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText
const cjs = { exports: {} }
new Function("module", "exports", output)(cjs, cjs.exports)
const { newModelApi, defaultModelApi, withUpstreamProtocol, selectUpstreamFormat, selectGatewayProtocol, normalizePastedUpstreamPath, upstreamFormatValue, upstreamFormatOptions, gatewayProtocolOptions, routeConfigError, upstreamAddress } = cjs.exports

test("upstream changes preserve the public contract and expose unsupported conversions", () => {
  const config = withUpstreamProtocol(newModelApi, "embeddings")
  assert.equal(config.api_type, "chat_completions")
  assert.match(routeConfigError(config, "").message, /无法接收/)
  const adapter = withUpstreamProtocol({ ...newModelApi, api_type: "messages" }, "chat_completions")
  assert.equal(routeConfigError(adapter, ""), null)
  assert.ok(upstreamFormatOptions(adapter, "").some(item => item.value === "custom:chat_completions"))
  assert.equal(routeConfigError(selectUpstreamFormat(adapter, "custom:chat_completions"), "/vendor/chat"), null)
})
test("1M marker only applies to Messages and clears when changing upstream format", () => {
  const messages = { ...newModelApi, api_type: "messages", upstream_api_type: "messages", context_1m: true }
  assert.equal(routeConfigError(messages, ""), null)
  assert.ok(routeConfigError(messages, "", true))
  const chat = selectUpstreamFormat(messages, "chat_completions")
  assert.equal(chat.context_1m, false)
  assert.equal(routeConfigError(chat, ""), null)
  assert.ok(routeConfigError({ ...chat, context_1m: true }, ""))
  assert.equal(selectUpstreamFormat(messages, "custom:messages").context_1m, true)
})
test("custom paths retain a visible wire format; switching the public API does not change it", () => {
  const config = selectUpstreamFormat({ ...newModelApi, api_type: "images_generations" }, "custom:images_generations")
  assert.equal(config.upstream_mode, "custom")
  assert.equal(routeConfigError(config, "/vendor/image"), null)
  const changed = { ...config, api_type: "audio_speech" }
  assert.equal(upstreamFormatValue(changed, "/vendor/image"), "custom:images_generations")
  assert.ok(upstreamFormatOptions(changed, "/vendor/image").some(item => item.value === "custom:images_generations"))
  assert.ok(routeConfigError(changed, "/vendor/image"))
  assert.equal(routeConfigError(selectUpstreamFormat(changed, "custom:audio_speech"), "/vendor/speech"), null)
  assert.equal(selectUpstreamFormat(config, ""), config)
  assert.equal(selectUpstreamFormat(config, "custom:unknown"), config)
})
test("unsafe paths, empty custom destinations and invalid task contracts cannot be submitted", () => {
  const custom = selectUpstreamFormat(newModelApi, "custom:chat_completions")
  for (const path of ["", "/", "//other.test/chat", "/../chat", "/chat?key=x", "/path with space", "/%2e%2e/admin"]) assert.equal(routeConfigError(custom, path).field, "model-request-path")
  assert.equal(routeConfigError({ ...custom, response_mode: "task", task_path: "/tasks/{task_id}" }, "/chat").field, "model-api-response")
  const media = { ...custom, api_type: "images_generations", upstream_api_type: "images_generations", response_mode: "task", task_path: "/tasks/{task_id}" }
  assert.equal(routeConfigError(media, "/generate"), null)
  assert.equal(routeConfigError({ ...media, task_path: "/tasks/{other}" }, "/generate").field, "model-task-path")
})
test("legacy destinations and engine routes keep their existing contracts", () => {
  assert.equal(upstreamFormatValue(defaultModelApi, ""), "auto")
  assert.equal(routeConfigError(defaultModelApi, ""), null)
  assert.equal(upstreamFormatValue({ ...newModelApi, upstream_mode: undefined }, "/old/chat"), "custom:chat_completions")
  assert.equal(routeConfigError({ ...defaultModelApi, api_type: "responses" }, "", true), null)
  assert.ok(routeConfigError({ ...defaultModelApi, api_type: "images_generations" }, "", true))
  assert.deepEqual(gatewayProtocolOptions(newModelApi, true).map(item => item.value), ["chat_completions", "responses"])
})
test("address formatting handles endpoint bases and preserves custom path independence", () => {
  const config = { ...newModelApi, api_type: "messages", upstream_api_type: "messages" }
  assert.equal(upstreamAddress("https://vendor.test/prefix/v1/messages/", "", config), "https://vendor.test/prefix/v1/messages")
  assert.equal(upstreamAddress("https://vendor.test/prefix/v1", "/native/messages", config), "https://vendor.test/native/messages")
})
test("changing the gateway to speech selects matching wire format and retains the supplier path", () => {
  const path = "/v1/audio/transcriptions"
  const initial = selectUpstreamFormat(newModelApi, "custom:chat_completions")
  const config = selectGatewayProtocol(initial, "audio_transcriptions", path)
  assert.equal(config.upstream_api_type, "audio_transcriptions")
  assert.equal(config.upstream_mode, "custom")
  assert.equal(routeConfigError(config, path), null)
  assert.equal(upstreamAddress("https://api.siliconflow.cn/v1", path, config), "https://api.siliconflow.cn/v1/audio/transcriptions")
  const empty = selectGatewayProtocol(newModelApi, "audio_transcriptions", "")
  assert.equal(routeConfigError(empty, "").field, "model-request-path")
  assert.equal(selectGatewayProtocol(newModelApi, "messages", "").upstream_api_type, "chat_completions")
  const messages = { ...newModelApi, api_type: "messages", upstream_api_type: "messages", context_1m: true }
  assert.equal(selectGatewayProtocol(messages, "audio_transcriptions", path).context_1m, false)
  const task = { ...config, response_mode: "task", task_path: "/tasks/{task_id}" }
  assert.equal(selectGatewayProtocol(task, "responses", path).response_mode, "direct")
  assert.equal(selectGatewayProtocol(task, "responses", path).task_path, "")
  assert.equal(selectGatewayProtocol(defaultModelApi, "responses", "", true).upstream_api_type, null)
})
test("pasted URLs cannot redirect a supplier key or normalize unsafe paths", () => {
  const base = "https://api.siliconflow.cn/v1"
  assert.equal(normalizePastedUpstreamPath("https://api.siliconflow.cn/v1/audio/transcriptions", base), "/v1/audio/transcriptions")
  for (const value of ["/v1/audio/transcriptions", "https://other.test/v1/audio/transcriptions", "http://api.siliconflow.cn/v1/audio/transcriptions", "https://user:password@api.siliconflow.cn/v1/audio/transcriptions", "https://api.siliconflow.cn/v1/../admin", "https://api.siliconflow.cn/%2e%2e/admin", "https://api.siliconflow.cn/v1/audio/transcriptions?key=x", "https://api.siliconflow.cn/v1/audio/transcriptions#fragment"]) {
    assert.equal(normalizePastedUpstreamPath(value, base), value)
  }
})
