import type { MonitorOverview, MonitorAlert, MonitorTask, MonitorAudit, MonitorList, MonitorSettings, AlertConfig } from "./monitoring";
import type { ModelApiConfig } from "@/lib/model-api";
import type { BillingConfig } from "@/lib/model-billing";
const API_BASE_URL = "";

export function getToken(): string | null {
  if (typeof window === "undefined") return null;
  return localStorage.getItem("hub_token");
}

export function setToken(token: string) {
  if (typeof window !== "undefined") localStorage.setItem("hub_token", token);
}

export function removeToken() {
  if (typeof window !== "undefined") localStorage.removeItem("hub_token");
}

export function setUser(user: unknown) {
  if (typeof window !== "undefined")
    localStorage.setItem("hub_user", JSON.stringify(user));
}

export function getUser<T = unknown>(): T | null {
  if (typeof window === "undefined") return null;
  const stored = localStorage.getItem("hub_user");
  if (!stored) return null;
  try {
    return JSON.parse(stored) as T;
  } catch {
    // 缓存损坏（旧版本格式、被外部改写、存储截断）不该让整个页面崩掉：
    // 丢掉坏数据返回 null，界面按“未登录”渲染，用户重新登录即可恢复。
    localStorage.removeItem("hub_user");
    return null;
  }
}

export function removeUser() {
  if (typeof window !== "undefined") localStorage.removeItem("hub_user");
}

interface APIResponse<T> {
  code: number;
  message?: string;
  reason?: string;
  field?: "email" | "password" | "verification_code";
  data: T;
}

export class APIError extends Error {
  status: number;
  reason?: string;
  field?: "email" | "password" | "verification_code";

  constructor(message: string, status: number, reason?: string, field?: "email" | "password" | "verification_code") {
    super(message);
    this.name = "APIError";
    this.status = status;
    this.reason = reason;
    this.field = field;
  }
}

async function request<T>(path: string, body: unknown = {}, { authenticated = true }: { authenticated?: boolean } = {}): Promise<T> {
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
  };
  const token = getToken();
  if (authenticated && token) headers.Authorization = `Bearer ${token}`;

  let response: Response;
  try {
    response = await fetch(`${API_BASE_URL}${path}`, {
      method: "POST",
      headers,
      body: JSON.stringify(body),
    });
  } catch {
    throw new Error("无法连接服务器，请检查网络后重试。");
  }
  const newToken = response.headers.get("X-Jwt-Token");
  if (newToken) setToken(newToken);

  if (response.status === 401 && authenticated) {
    removeToken();
    removeUser();
    if (typeof window !== "undefined") window.location.replace("/login");
    throw new Error("未授权");
  }

  const text = await response.text();
  let result: APIResponse<T>;
  try {
    result = JSON.parse(text) as APIResponse<T>;
  } catch {
    throw new Error(`服务器返回了无效数据: ${text.slice(0, 100)}`);
  }
  if (!response.ok || result.code !== 200) {
    const message = result.message?.trim();
    if (path === "/auth/login" && (response.status === 401 || result.code === 401)) {
      throw new APIError(message && message !== "未授权" ? message : "登录验证未通过，请重试。", response.status, result.reason, result.field);
    }
    throw new APIError(message || "请求失败，请稍后重试。", response.status, result.reason, result.field);
  }
  return result.data;
}

export interface AuthResponse {
  token: string;
  uid: number | string;
  nickname: string;
  email?: string;
  avatar_url?: string;
  role: "admin" | "user";
}

export interface DispatchPolicy {
  strategy: "balanced" | "priority" | "round_robin" | "weighted";
  max_attempts: number;
  sticky_sessions: boolean;
  request_timeout_seconds: number;
  failure_cooldown_seconds: number;
}

export interface ConnectionTestResult {
  request_path?: string;
  models?: string[];
  ok: boolean;
  outcome: "success" | "warning" | "error";
  status_code: number;
  message: string;
  model_count?: number;
  latency_ms?: number;
  suggested_base_url?: string | null;
  checks?: Array<ConnectionTestResult & { name: string }>;
  account_name?: string;
}

export interface ConsoleProvider {
  id: number;
  name: string;
  provider_type: string;
  base_url: string;
  avatar_url?: string;
  has_api_key: boolean;
  managed?: boolean;
  platform?: string | null;
  enabled: number;
}

export interface ConsoleProviderAccount {
  id: number;
  name: string;
  platform: string;
  kind: string;
  priority: number;
  member_order?: number;
  allowed_models?: string[];
  weight?: number;
  concurrency?: number;
  rpm?: number;
  health?: "ready" | "expired" | "disabled" | "rate_limited" | "cooling";
  cooldown_until?: number | null;
  status: string;
  schedulable: boolean;
  base_url?: string | null;
  has_api_key: boolean;
  oauth_type?: string | null;
  project_id?: string | null;
  client_id?: string | null;
  has_client_secret: boolean;
}

export interface ConsoleModel {
  billing_config?: BillingConfig;
  request_path?: string;
  api_config?: ModelApiConfig;
  input_price_per_million?: number | null;
  output_price_per_million?: number | null;
  id: number;
  provider_id: number;
  provider_name?: string;
  provider_type?: string;
  alias: string;
  upstream_model: string;
  capabilities: string[];
  enabled: number;
}

export interface ConsoleLog {
  request_id?: string;
  cost_usd?: number | null;
  billing_details?: { unit?: BillingConfig["unit"]; quantity?: string; price_per_unit?: string; amount_usd?: string; unpriced_reason?: string };
  id: number;
  api_key_id: number;
  key_name: string;
  owner_id?: number;
  owner_name?: string;
  owner_email?: string;
  model_alias: string;
  upstream_model: string;
  provider_id?: number;
  provider_name?: string;
  provider_type: string;
  status_code: number;
  duration_ms: number;
  error_message: string;
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  create_time: number;
}

export interface ConsoleLogQuery {
  request_id?: string;
  page?: number;
  page_size?: number;
  q?: string;
  user_id?: number;
  user_search?: string;
  model?: string;
  provider?: string;
  status?: "success" | "4xx" | "5xx" | "other" | "";
  from?: number;
  to?: number;
}

export interface ConsoleLogFilters {
  users: Array<{ id: number; name: string; email: string }>;
  models: string[];
  providers: string[];
}

export interface ConsoleKey {
  owner_id?: number;
  owner_name?: string;
  owner_email?: string;
  can_reveal?: boolean;
  cost_usd?: number;
  unpriced_requests?: number;
  id: number;
  name: string;
  key?: string;
  key_prefix: string;
  key_masked: string;
  allowed_models: string[];
  status: number;
  expires_at: number;
  total_requests?: number;
  input_tokens?: number;
  output_tokens?: number;
}

export const authAPI = {
  login: (data: { email: string; password: string }) =>
    request<AuthResponse>("/auth/login", data, { authenticated: false }),
  register: (data: {
    email: string;
    password: string;
    verification_code: string;
    display_name?: string;
  }) => request<AuthResponse>("/auth/register", data, { authenticated: false }),
  sendRegistrationCode: (email: string) => request<{ retry_after: number; expires_in: number }>("/auth/register/code", { email }, { authenticated: false }),
};

export interface AccountProfile {
  uid: number | string;
  nickname: string;
  display_name: string;
  email: string;
  avatar_url: string;
  role: "admin" | "user";
}

export const accountAPI = {
  profile: () => request<AccountProfile>("/account-api/profile"),
  updateProfile: (data: { display_name?: string; avatar_url?: string }) =>
    request<AccountProfile>("/account-api/profile/update", data),
  updatePassword: (data: { current_password: string; new_password: string }) =>
    request<Record<string, never>>("/account-api/password/update", data),
};

export interface AccessGuide { template_content?: string; content: string; revision: number; updated_at: number }

export type RequestLimits = { max_concurrency: number; rpm: number };

export const consoleAPI = {
  availableModels: () => request<{list: AvailableModel[]; total: number}>("/console-api/model/available"),
  accessGuide: () => request<AccessGuide>("/console-api/guide/get"),
  saveAccessGuide: (data: {content: string; revision: number}) => request<AccessGuide>("/hub_admin/guide/update", data),
  profile: () => request<Record<string, unknown>>("/console-api/profile"),
  stats: (since?: number) =>
    request<{
      providers?: number;
      models: number;
      keys: number;
      requests: number;
      input_tokens: number;
      output_tokens: number;
      total_tokens: number;
      total_requests?: number;
      cost_usd?: number;
      unpriced_requests?: number;
    }>("/console-api/stats", { since }),
  createSub2apiApiKeyAccount: (data: {
    target_provider_id?: number;
    platform: string;
    name: string;
    api_key: string;
    base_url: string;
  }) =>
    request<{ provider_id: number }>(
      "/console-api/sub2api/accounts/api-key",
      data,
    ),
  startOpenAIOAuth: () =>
    request<{ auth_url: string; session_id: string; state: string }>(
      "/console-api/sub2api/oauth/openai/start",
    ),
  completeOpenAIOAuth: (data: {
    session_id: string;
    state: string;
    code: string;
    name?: string;
    target_provider_id?: number;
    provider_id?: number;
    account_id?: number;
  }) =>
    request<{ provider_id: number; account_id?: number; updated?: boolean }>(
      "/console-api/sub2api/oauth/openai/complete",
      data,
    ),
  startProviderOAuth: (
    platform: string,
    data: {
      oauth_type?: string;
      project_id?: string;
      client_id?: string;
      client_secret?: string;
    } = {},
  ) =>
    request<{ auth_url: string; session_id: string; state: string }>(
      `/console-api/sub2api/oauth/${platform}/start`,
      data,
    ),
  completeProviderOAuth: (
    platform: string,
    data: {
      session_id: string;
      state: string;
      code: string;
      name?: string;
      target_provider_id?: number;
      provider_id?: number;
      account_id?: number;
    },
  ) =>
    request<{ provider_id: number; account_id?: number; updated?: boolean }>(
      `/console-api/sub2api/oauth/${platform}/complete`,
      data,
    ),
  importProviderOAuthToken: (
    platform: string,
    data: {
      target_provider_id?: number;
      name?: string;
      refresh_token?: string;
      access_token?: string;
      expires_in?: number;
      oauth_type?: string;
      project_id?: string;
      client_id?: string;
      client_secret?: string;
    },
  ) =>
    request<{ provider_id: number }>(
      `/console-api/sub2api/oauth/${platform}/import`,
      data,
    ),
  providerList: () =>
    request<{ list: ConsoleProvider[]; total: number }>(
      "/console-api/provider/list",
    ),
  providerCreate: (data: {
    name: string;
    provider_type: string;
    base_url: string;
    api_key: string;
    avatar_url?: string;
    request_limits?: RequestLimits;
  }) => request<ConsoleProvider>("/console-api/provider/create", data),
  providerUpdate: (data: {
    id: number;
    name?: string;
    provider_type?: string;
    base_url?: string;
    api_key?: string;
    avatar_url?: string;
    enabled?: number;
    request_limits?: RequestLimits;
  }) => request<ConsoleProvider>("/console-api/provider/update", data),
  providerDelete: (id: number) =>
    request<Record<string, never>>("/console-api/provider/delete", { id }),
  providerTest: (id: number, model?: string, settings?: { api_type: "chat_completions" | "responses" | "messages"; auth_type: "bearer" | "x_api_key"; context_1m?: boolean }) =>
    request<ConnectionTestResult>("/console-api/provider/test", {
      id,
      ...(model ? { model, ...settings } : {}),
    }),
  modelTest: (data: { provider_id: number; upstream_model: string; request_path: string; api_config: ModelApiConfig }) =>
    request<ConnectionTestResult>("/console-api/model/test", data),
  modelPreview: (data: { provider_id: number; upstream_model: string; request_path: string; api_config: ModelApiConfig }) =>
    request<{ managed: boolean; upstream_url: string | null; available_accounts: number | null }>("/console-api/model/preview", data),
  testConnection: (data: {
    base_url: string;
    api_key: string;
    platform: string;
    model?: string;
    api_type?: "chat_completions" | "responses" | "messages";
    auth_type?: "bearer" | "x_api_key";
    context_1m?: boolean;
  }) =>
    request<ConnectionTestResult>(
      "/console-api/provider/test-connection",
      data,
    ),
  providerModels: (id: number) =>
    request<{ models: string[]; total: number }>(
      "/console-api/provider/models",
      { id },
    ),
  providerPolicy: (id: number) =>
    request<DispatchPolicy>("/console-api/provider/policy", { id }),
  providerPolicyUpdate: (id: number, policy: DispatchPolicy) =>
    request<DispatchPolicy>("/console-api/provider/policy/update", {
      id,
      ...policy,
    }),
  resourceLimits: (kind: "provider" | "model" | "key" | "user", id: number) =>
    request<{ max_concurrency: number; rpm: number }>(
      `/console-api/${kind}/limits`,
      { id },
    ),
  resourceLimitsUpdate: (
    kind: "provider" | "model" | "key" | "user",
    id: number,
    value: { max_concurrency: number; rpm: number },
  ) =>
    request<{ max_concurrency: number; rpm: number }>(
      `/console-api/${kind}/limits/update`,
      { id, ...value },
    ),
  providerAccountCreate: (
    providerId: number,
    value: { name: string; base_url?: string; api_key: string },
  ) =>
    request<{ id: number }>("/console-api/provider/account/create", {
      provider_id: providerId,
      ...value,
    }),
  providerAccounts: (providerId: number) =>
    request<{ list: ConsoleProviderAccount[]; total: number }>(
      "/console-api/provider/accounts",
      { provider_id: providerId },
    ),
  providerAccountPrioritiesUpdate: (
    providerId: number,
    accounts: Array<{ id: number; priority: number }>,
  ) =>
    request<{ updated: number }>("/console-api/provider/accounts/update", {
      provider_id: providerId,
      accounts,
    }),
  providerAccountDispatch: (
    providerId: number,
    accounts: Array<{
      id: number;
      priority: number;
      member_order?: number;
      weight: number;
      concurrency: number;
      rpm: number;
      allowed_models: string[];
    }>,
  ) =>
    request<{ updated: number }>("/console-api/provider/accounts/dispatch", {
      provider_id: providerId,
      accounts,
    }),
  providerAccountDelete: (providerId: number, accountId: number) =>
    request<{ deleted: boolean }>("/console-api/provider/account/delete", {
      provider_id: providerId,
      account_id: accountId,
    }),
  providerAccountUpdate: (
    providerId: number,
    data: {
      account_id: number;
      name: string;
      base_url?: string;
      api_key?: string;
    },
  ) =>
    request<{ updated: boolean }>("/console-api/provider/account/update", {
      provider_id: providerId,
      ...data,
    }),
  modelList: () =>
    request<{ list: ConsoleModel[]; total: number }>("/console-api/model/list"),
  modelCreate: (data: {
    billing_config?: BillingConfig;
    request_path?: string;
  api_config?: ModelApiConfig;
    input_price_per_million?: number | null;
    output_price_per_million?: number | null;
    provider_id: number;
    alias: string;
    upstream_model: string;
    capabilities?: string[];
    request_limits?: RequestLimits;
  }) => request<ConsoleModel>("/console-api/model/create", data),
  modelImport: (data: { provider_id: number; models: string[] }) =>
    request<{
      imported: string[];
      skipped: string[];
      imported_count: number;
      skipped_count: number;
    }>("/console-api/model/import", data),
  modelUpdate: (data: {
    billing_config?: BillingConfig;
    request_path?: string;
  api_config?: ModelApiConfig;
    input_price_per_million?: number | null;
    output_price_per_million?: number | null;
    id: number;
    provider_id?: number;
    alias?: string;
    upstream_model?: string;
    capabilities?: string[];
    enabled?: number;
    request_limits?: RequestLimits;
  }) => request<ConsoleModel>("/console-api/model/update", data),
  modelDelete: (id: number) =>
    request<Record<string, never>>("/console-api/model/delete", { id }),
  keyReveal: (id: number) =>
    request<{ key: string }>("/console-api/key/reveal", { id }),
  keyList: () =>
    request<{ list: ConsoleKey[]; total: number; scope: "all" | "own" }>(
      "/console-api/key/list",
    ),
  keyOwnList: () =>
    request<{ list: ConsoleKey[]; total: number; scope: "own" }>(
      "/console-api/key/own-list",
    ),
  keyCreate: (data: {
    name: string;
    allowed_models?: string[];
    expires_at?: number;
    request_limits?: RequestLimits;
  }) => request<ConsoleKey>("/console-api/key/create", data),
  keyUpdate: (data: {
    id: number;
    name?: string;
    allowed_models?: string[];
    status?: number;
    expires_at?: number;
    request_limits?: RequestLimits;
  }) => request<ConsoleKey>("/console-api/key/update", data),
  keyDelete: (id: number) =>
    request<Record<string, never>>("/console-api/key/delete", { id }),
  clientConnectCreate: (data: {
    key_id: number;
    model: string;
    client: "claude" | "codex" | "gemini" | "all";
  }) =>
    request<{ token: string; expires_at: number }>(
      "/console-api/client-connect/create",
      data,
    ),
  integrationList: () =>
    request<{ list: IntegrationProfile[]; total: number }>(
      "/console-api/integration/list",
    ),
  integrationDetail: (id: string) =>
    request<{ settingsConfig: Record<string, unknown> | null }>(
      "/console-api/integration/detail",
      { id },
    ),
  integrationUpsert: (data: IntegrationProfileInput) =>
    request<{ id: string }>("/console-api/integration/upsert", data),
  integrationDelete: (id: string) =>
    request<{ deleted: boolean }>("/console-api/integration/delete", { id }),
  integrationSync: (id: string, client?: IntegrationClient) =>
    request<{ token: string; expires_at: number }>(
      "/console-api/integration/sync",
      { id, client },
    ),
  integrationTest: (id: string, client: IntegrationClient) =>
    request<ConnectionTestResult>("/console-api/integration/test", {
      id,
      client,
    }),
  integrationReorder: (ids: string[]) =>
    request<{ updated: number }>("/console-api/integration/reorder", { ids }),
  logs: (query: ConsoleLogQuery = {}) =>
    request<{
      list: ConsoleLog[];
      total: number;
      page: number;
      page_size: number;
      scope: "all" | "own";
      filters: ConsoleLogFilters;
    }>("/console-api/logs", query),
};

export interface AvailableModel {
  request_formats?: string[];
  billing_config?: Pick<BillingConfig, "unit" | "price_per_unit">;
  icon?: Pick<ConsoleProvider, "provider_type" | "platform" | "avatar_url">;
  alias: string;
  api_type: string;
  endpoint: string;
  response_mode: "direct" | "task";
  capabilities: string[];
  input_price_per_million: number | null;
  output_price_per_million: number | null;
}

export type IntegrationClient =
  | "claude"
  | "codex"
  | "gemini"
  | "grok"
  | "opencode"
  | "openclaw"
  | "hermes"
  | "pi";
export type IntegrationApps = Record<IntegrationClient, boolean>;
export type IntegrationModels = {
  claude?: {
    model: string;
    haikuModel?: string;
    sonnetModel?: string;
    opusModel?: string;
  };
  codex?: { model: string; reasoningEffort?: string };
  gemini?: { model: string };
  grok?: { model: string; protocol?: string };
  opencode?: { model: string; protocol?: string };
  openclaw?: { model: string; protocol?: string };
  hermes?: { model: string; protocol?: string };
  pi?: { model: string; protocol?: string };
};
export type IntegrationProfileInput = {
  id?: string;
  name: string;
  providerType: "junohub" | "newapi" | "custom_gateway";
  keyId: number;
  apps: IntegrationApps;
  models: IntegrationModels;
  baseUrl?: string;
  apiKey?: string;
  copyFromId?: string;
  websiteUrl?: string;
  notes?: string;
  metadata?: {
    scope?: "client" | "universal";
    nativeClient?: IntegrationClient;
    apiFormat?: string;
    commonConfigEnabled?: boolean;
    pricing?: Record<string,number>;
    customEndpoints?: Record<
      string,
      { url: string; addedAt: number; lastUsed?: number }
    >;
    codexChatReasoning?: Record<string, unknown>;
    authBinding?: {
      authProvider: string;
      accountId?: string;
      accountName?: string;
    };
    providerKey?: string;
    category?: string;
    icon?: string;
    presetId?: string;
    junohubKeyId?: number;
  };
  settingsConfig?: Record<string, unknown>;
};
export type IntegrationProfile = IntegrationProfileInput & {
  id: string;
  keyName: string;
  keyPrefix: string;
  keyStatus: number;
  baseUrl: string;
  hasApiKey: boolean;
  createdAt: number;
  updatedAt: number;
  sortOrder: number;
  hasNativeConfig?: boolean;
};

export type AdminUserDetail = {
  user: {id:number; email:string; display_name:string; role:string; status:number; created_at:number; updated_at:number};
  usage: {keys_count:number; requests:number; input_tokens:number; output_tokens:number; cost_usd:number; unpriced_requests:number; last_used_at:number; models_count:number; providers_count:number};
  keys: Array<{id:number; name:string; prefix:string; status:number; expires_at:number; requests:number; input_tokens:number; output_tokens:number; cost_usd:number}>;
};

export const adminUserAPI = {
  detail: (id:number) => request<AdminUserDetail>("/hub_admin/users/detail", {id}),
  list: (page = 1, pageSize = 50) =>
    request<{
      list: Array<{
        id: number;
        email: string;
        display_name: string;
        role: "admin" | "user";
        status: number;
        create_time: number;
      }>;
      total: number;
    }>("/hub_admin/users/list", { page, page_size: pageSize }),
  update: (data: { id: number; status?: number; role?: "admin" | "user"; request_limits?: RequestLimits }) =>
    request<{ id: number; status: number; role: "admin" | "user" }>(
      "/hub_admin/users/update",
      data,
    ),
};


export const monitorAPI = {
  overview: (hours: number) => request<MonitorOverview>("/console-api/monitor/overview", { hours }),
  settings: () => request<MonitorSettings>("/console-api/monitor/settings"),
  saveSettings: (config: AlertConfig, revision: number) => request<MonitorSettings>("/console-api/monitor/settings/update", { config, revision }),
  alerts: (scope: "active" | "history", page: number) => request<MonitorList<MonitorAlert>>("/console-api/monitor/alerts", { scope, page }),
  acknowledge: (id: number) => request<{ acknowledged: boolean }>("/console-api/monitor/alerts/acknowledge", { id }),
  tasks: (status: string, page: number) => request<MonitorList<MonitorTask>>("/console-api/monitor/tasks", { status, page }),
  recheckTask: (id: string) => request<{ scheduled: boolean }>("/console-api/monitor/tasks/recheck", { id }),
  audit: (page: number) => request<MonitorList<MonitorAudit>>("/console-api/monitor/audit", { page }),
};
