export const modelInterfaces = [
  { value: "chat_completions", label: "OpenAI 格式 · Chat Completions", endpoint: "/v1/chat/completions", capability: "chat" },
  { value: "responses", label: "OpenAI 格式 · Responses", endpoint: "/v1/responses", capability: "chat" },
  { value: "messages", label: "Anthropic 格式 · Messages", endpoint: "/v1/messages", capability: "chat" },
] as const

// JunoHub owns these endpoints; they are not supplier endpoint presets.
export const gatewayModelInterfaces = [
  ...modelInterfaces,
  { value: "images_generations", label: "图片生成", endpoint: "/v1/images/generations", capability: "image" },
  { value: "images_edits", label: "图片编辑", endpoint: "/v1/images/edits", capability: "image" },
  { value: "videos_generations", label: "视频生成", endpoint: "/v1/videos/generations", capability: "video" },
  { value: "audio_generations", label: "音频生成", endpoint: "/v1/audio/generations", capability: "audio" },
  { value: "audio_speech", label: "语音合成 · Speech", endpoint: "/v1/audio/speech", capability: "audio" },
  { value: "audio_transcriptions", label: "语音识别", endpoint: "/v1/audio/transcriptions", capability: "audio" },
  { value: "audio_translations", label: "语音翻译", endpoint: "/v1/audio/translations", capability: "audio" },
  { value: "embeddings", label: "向量化 · Embeddings", endpoint: "/v1/embeddings", capability: "embedding" },
  { value: "search", label: "搜索", endpoint: "/v1/search", capability: "search" },
  { value: "rerank", label: "文本重排", endpoint: "/v1/rerank", capability: "rerank" },
] as const
const wireLabel = (label: string) => label.replace(/^(OpenAI|Anthropic) 格式 · /, "")
const automaticInterface = { value: "auto", label: "自动 · 兼容现有调用", endpoint: "", capability: "chat" } as const

export type ModelApiType = typeof gatewayModelInterfaces[number]["value"] | "auto"
export interface ModelApiConfig {
  api_type: ModelApiType
  upstream_api_type?: ModelApiType | null
  upstream_mode?: "preset" | "custom" | null
  auth_type: "bearer" | "x_api_key"
  context_1m?: boolean
  response_mode: "direct" | "task"
  task_path: string
}
export const defaultModelApi: ModelApiConfig = { api_type: "auto", upstream_api_type: null, auth_type: "bearer", response_mode: "direct", task_path: "" }
export const newModelApi: ModelApiConfig = { ...defaultModelApi, api_type: "chat_completions", upstream_api_type: "chat_completions", upstream_mode: "preset" }
export function modelInterface(type?: string) { return gatewayModelInterfaces.find(item => item.value === type) || automaticInterface }
export function upstreamInterface(config?: ModelApiConfig) { return modelInterface(config?.upstream_api_type && config.upstream_api_type !== "auto" ? config.upstream_api_type : config?.api_type) }
export function isProtocolPairSupported(gateway: string, upstream: string) {
  return gateway === upstream || (gateway === "messages" && upstream === "chat_completions")
}
export function gatewayProtocolOptions(config: ModelApiConfig, managed = false) {
  const options = managed ? gatewayModelInterfaces.filter(item => ["chat_completions", "responses"].includes(item.value)) : [...gatewayModelInterfaces]
  return config.api_type === "auto" ? [...options, { ...automaticInterface, disabled: true }] : options
}
// Only the upstream is changed. A mismatched public contract must be resolved explicitly.
export function withUpstreamProtocol(config: ModelApiConfig, upstream: ModelApiType): ModelApiConfig {
  return { ...config, upstream_api_type: upstream, ...(config.context_1m && upstream !== "messages" ? { context_1m: false } : {}) }
}
export function upstreamFormatValue(config: ModelApiConfig, path: string) {
  const upstream = upstreamInterface(config).value
  const custom = config.upstream_mode === "custom" || (!config.upstream_mode && (Boolean(path.trim()) || (upstream !== "auto" && !modelInterfaces.some(item => item.value === upstream))))
  return custom ? `custom:${upstream}` : upstream
}
export function upstreamFormatOptions(config: ModelApiConfig, path: string) {
  const value = upstreamFormatValue(config, path)
  const customTypes = gatewayModelInterfaces.filter(item => isProtocolPairSupported(config.api_type, item.value))
  const customOptions = customTypes.map(item => ({ value: `custom:${item.value}`, label: `自定义路径 · ${wireLabel(item.label)}` }))
  return [
    ...modelInterfaces,
    ...customOptions,
    ...(value.startsWith("custom:") && !customOptions.some(item => item.value === value) && config.api_type !== "auto" ? [{ value, label: `自定义路径 · ${wireLabel(upstreamInterface(config).label)}` }] : []),
    ...(config.api_type === "auto" ? [{ value, label: "自动转发 · 原有配置", disabled: true }] : []),
  ]
}
export function selectUpstreamFormat(config: ModelApiConfig, value: string): ModelApiConfig {
  const custom = value.startsWith("custom:")
  const upstream = custom ? value.slice(7) : value
  if (!gatewayModelInterfaces.some(item => item.value === upstream)) return config
  return { ...withUpstreamProtocol(config, upstream as ModelApiType), upstream_mode: custom ? "custom" : "preset" }
}
// Changing a public interface must not leave an incompatible upstream wire format.
// Keep supported adapters and custom destinations; never guess a supplier's path.
export function selectGatewayProtocol(config: ModelApiConfig, gateway: ModelApiType, path: string, managed = false): ModelApiConfig {
  if (managed || isProtocolPairSupported(gateway, upstreamInterface(config).value)) return { ...config, api_type: gateway }
  const dialogue = modelInterfaces.some(item => item.value === gateway)
  return {
    ...withUpstreamProtocol(config, gateway), api_type: gateway,
    upstream_mode: path.trim() || config.upstream_mode === "custom" || !dialogue ? "custom" : "preset",
    ...(dialogue ? { response_mode: "direct", task_path: "" } : {}),
  }
}
// Full URLs are accepted only as a paste convenience for the selected supplier.
// Validate the original path before URL parsing can normalize dot segments.
export function normalizePastedUpstreamPath(value: string, providerBaseUrl: string): string {
  const candidate = value.trim()
  const match = candidate.match(/^https?:\/\/[^/?#]+(\/[^?#]*)$/i)
  if (!match || requestPathError(match[1])) return value
  try {
    const url = new URL(candidate)
    const provider = new URL(providerBaseUrl)
    if (url.username || url.password || url.origin !== provider.origin) return value
    return match[1]
  } catch { return value }
}
export function protocolPairError(config: ModelApiConfig) {
  const upstream = upstreamInterface(config).value
  return isProtocolPairSupported(config.api_type, upstream) ? "" : `上游为${modelInterface(upstream).label}，无法接收${modelInterface(config.api_type).label}请求。请调整上游请求格式。`
}
export function requestPathError(path: string) {
  if (!path) return "请填写上游请求路径。"
  if (path.length > 1024 || !/^\/[\x21-\x7e]+$/.test(path) || path.startsWith("//") || /[?#\\%]/.test(path) || path.includes("://") || path.split("/").some(part => part === "." || part === "..")) return "路径须以 / 开头，不能包含域名、查询参数、空格或相对路径。"
  return ""
}
export function routeConfigError(config: ModelApiConfig, path: string, managed = false): { field: string; message: string } | null {
  if (config.context_1m && (managed || upstreamInterface(config).value !== "messages")) return { field: "model-upstream-api-type", message: "1M 上下文标记仅用于 Messages 上游。" }
  if (managed) return ["auto", "chat_completions", "responses"].includes(config.api_type) ? null : { field: "model-api-type", message: "账户引擎目前支持 Chat Completions 和 Responses 入口。" }
  const pairError = protocolPairError(config)
  if (pairError) return { field: "model-upstream-api-type", message: pairError }
  const custom = upstreamFormatValue(config, path).startsWith("custom:")
  const pathError = custom ? requestPathError(path.trim()) : path.trim() ? "预设连接不能覆盖路径，请选择自定义路径。" : ""
  if (pathError) return { field: "model-request-path", message: pathError }
  if (config.response_mode === "task") {
    if (["auto", "chat_completions", "responses", "messages"].includes(upstreamInterface(config).value)) return { field: "model-api-response", message: "对话格式不支持任务返回方式，请改为直接响应。" }
    const template = config.task_path.trim()
    if (template.split("{task_id}").length !== 2 || /[{}]/.test(template.replace("{task_id}", "task")) || requestPathError(template.replace("{task_id}", "task"))) return { field: "model-task-path", message: "请填写合法的查询路径，包含一个 {task_id}。" }
  } else if (config.task_path.trim()) return { field: "model-api-response", message: "直接响应不需要任务查询路径。" }
  return null
}
export function upstreamAddress(baseUrl: string, requestPath: string, config?: ModelApiConfig) {
  try {
    const url = new URL(baseUrl)
    if (requestPath) return url.origin + requestPath
    const endpoint = upstreamInterface(config).endpoint
    if (!endpoint) return ""
    const prefix = url.pathname.replace(/\/+$/, "").replace(/\/(chat\/completions|responses|messages|models)$/, "")
    return url.origin + prefix + (prefix.endsWith("/v1") ? endpoint.slice(3) : endpoint)
  } catch { return requestPath }
}
