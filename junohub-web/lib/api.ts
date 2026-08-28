const API_BASE_URL = ""

export function getToken(): string | null {
  if (typeof window === "undefined") return null
  return localStorage.getItem("hub_token")
}

export function setToken(token: string) {
  if (typeof window !== "undefined") localStorage.setItem("hub_token", token)
}

export function removeToken() {
  if (typeof window !== "undefined") localStorage.removeItem("hub_token")
}

export function setUser(user: unknown) {
  if (typeof window !== "undefined") localStorage.setItem("hub_user", JSON.stringify(user))
}

export function getUser<T = unknown>(): T | null {
  if (typeof window === "undefined") return null
  const stored = localStorage.getItem("hub_user")
  return stored ? JSON.parse(stored) as T : null
}

export function removeUser() {
  if (typeof window !== "undefined") localStorage.removeItem("hub_user")
}

interface APIResponse<T> {
  code: number
  message?: string
  data: T
}

async function request<T>(path: string, body: unknown = {}): Promise<T> {
  const headers: Record<string, string> = { "Content-Type": "application/json" }
  const token = getToken()
  if (token) headers.Authorization = `Bearer ${token}`

  const response = await fetch(`${API_BASE_URL}${path}`, {
    method: "POST",
    headers,
    body: JSON.stringify(body),
  })
  const newToken = response.headers.get("X-Jwt-Token")
  if (newToken) setToken(newToken)

  if (response.status === 401) {
    removeToken()
    removeUser()
    if (typeof window !== "undefined") window.location.href = "/login"
    throw new Error("未授权")
  }

  const text = await response.text()
  let result: APIResponse<T>
  try {
    result = JSON.parse(text) as APIResponse<T>
  } catch {
    throw new Error(`服务器返回了无效数据: ${text.slice(0, 100)}`)
  }
  if (!response.ok || result.code !== 200) throw new Error(result.message || "请求失败")
  return result.data
}

export interface AuthResponse {
  token: string
  uid: number | string
  nickname: string
  email?: string
  role: "admin" | "user"
}

export interface ConsoleProvider {
  id: number
  name: string
  provider_type: string
  base_url: string
  has_api_key: boolean
  enabled: number
}

export interface ConsoleModel {
  id: number
  provider_id: number
  provider_name?: string
  provider_type?: string
  alias: string
  upstream_model: string
  capabilities: string[]
  enabled: number
}

export interface ConsoleKey {
  id: number
  name: string
  key?: string
  key_prefix: string
  key_masked: string
  allowed_models: string[]
  status: number
  expires_at: number
  total_requests?: number
}

export const authAPI = {
  login: (data: { email: string; password: string }) => request<AuthResponse>("/auth/login", data),
  register: (data: { email: string; password: string; display_name?: string }) => request<AuthResponse>("/auth/register", data),
}

export const consoleAPI = {
  profile: () => request<Record<string, unknown>>("/console-api/profile"),
  stats: () => request<{ providers: number; models: number; keys: number; requests: number }>("/console-api/stats"),
  providerList: () => request<{ list: ConsoleProvider[]; total: number }>("/console-api/provider/list"),
  providerCreate: (data: { name: string; provider_type: string; base_url: string; api_key: string }) => request<ConsoleProvider>("/console-api/provider/create", data),
  providerUpdate: (data: { id: number; name?: string; provider_type?: string; base_url?: string; api_key?: string; enabled?: number }) => request<ConsoleProvider>("/console-api/provider/update", data),
  providerDelete: (id: number) => request<Record<string, never>>("/console-api/provider/delete", { id }),
  providerTest: (id: number) => request<{ ok: boolean; status_code: number }>("/console-api/provider/test", { id }),
  modelList: () => request<{ list: ConsoleModel[]; total: number }>("/console-api/model/list"),
  modelCreate: (data: { provider_id: number; alias: string; upstream_model: string; capabilities?: string[] }) => request<ConsoleModel>("/console-api/model/create", data),
  modelUpdate: (data: { id: number; provider_id?: number; alias?: string; upstream_model?: string; capabilities?: string[]; enabled?: number }) => request<ConsoleModel>("/console-api/model/update", data),
  modelDelete: (id: number) => request<Record<string, never>>("/console-api/model/delete", { id }),
  keyList: () => request<{ list: ConsoleKey[]; total: number }>("/console-api/key/list"),
  keyCreate: (data: { name: string; allowed_models?: string[]; expires_at?: number }) => request<ConsoleKey>("/console-api/key/create", data),
  keyUpdate: (data: { id: number; name?: string; allowed_models?: string[]; status?: number; expires_at?: number }) => request<ConsoleKey>("/console-api/key/update", data),
  keyDelete: (id: number) => request<Record<string, never>>("/console-api/key/delete", { id }),
  logs: (page = 1, pageSize = 20) => request<{ list: Array<Record<string, unknown>>; total: number }>("/console-api/logs", { page, page_size: pageSize }),
}

export const adminUserAPI = {
  list: (page = 1, pageSize = 50) => request<{ list: Array<{ id: number; email: string; display_name: string; role: "admin" | "user"; status: number; create_time: number }>; total: number }>("/hub_admin/users/list", { page, page_size: pageSize }),
  update: (data: { id: number; status?: number; role?: "admin" | "user" }) => request<{ id: number; status: number; role: "admin" | "user" }>("/hub_admin/users/update", data),
}
