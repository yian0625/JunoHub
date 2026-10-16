"use client"

import { parseOAuthResult } from "@/lib/oauth-result"

import { ConnectionTestResultView } from "@/components/connection-test-result"
import { UpstreamModelPicker } from "@/components/upstream-model-picker"
import { ModelTestSettings, defaultModelTestSettings } from "@/components/model-test-settings"
import { ConnectionTest } from "@/components/connection-test"
import { RequestLimitsFields, useRequestLimits } from "@/components/resource-limits-fields"
import { RoutingAccountsEditor, RoutingDraft } from "@/components/routing-accounts-editor"
import { AccountDispatchEditor } from "@/components/account-dispatch-editor"
import { DeleteProviderDialog } from "@/components/delete-provider-dialog"
import { AccountHealth } from "@/components/account-health"
import { DispatchPolicyEditor } from "@/components/dispatch-policy-editor"
import { DeleteProviderAccountDialog } from "@/components/delete-provider-account-dialog"
import { AppForm } from "@/components/ui/app-form"
import { ProviderAvatar } from "@/components/provider-avatar"
import { ProviderAvatarEditor } from "@/components/provider-avatar-editor"

import { useEffect, useRef, useState } from "react"
import { Check, ChevronDown, ChevronLeft, ExternalLink, KeyRound, Pencil, Plus, RotateCw, SearchIcon, Server, Settings2, Sparkles, Trash2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Button as CossButton } from "@/components/coss-ui/button"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/coss-ui/input-group"
import { SelectField } from "@/components/ui/select-field"
import { Input } from "@/components/ui/input"
import { ConfirmDialog } from "@/components/confirm-dialog"
import { ConsoleProvider, ConsoleProviderAccount, ConnectionTestResult, DispatchPolicy, consoleAPI } from "@/lib/api"
import { cn } from "@/lib/utils"
import { toast } from "sonner"
import { EmptyGrid, ErrorState, Field, FormDialog, FormError, LoadingState, PageHead, ResourceCard, ResourceGrid, NoSearchResults } from "../_components/console-ui"
import { useConsoleGuard } from "../_components/use-console-guard"

const providerTemplates = [
  { id: "sub2api", label: "Sub2API", description: "账户池、调度与故障切换", icon: Sparkles },
  { id: "openai", label: "OpenAI", description: "OpenAI 官方接口", icon: OpenAIIcon },
  { id: "openai-compatible", label: "兼容服务", description: "其他 OpenAI 兼容端点", icon: Server },
] as const

const accountPlatforms = [
  { id: "openai", label: "OpenAI", description: "OAuth 或 API Key", baseUrl: "https://api.openai.com/v1", icon: OpenAIIcon },
  { id: "anthropic", label: "Claude", description: "OAuth、Token 或 API Key", baseUrl: "https://api.anthropic.com", icon: ClaudeIcon },
  { id: "gemini", label: "Gemini", description: "Google OAuth 或 API Key", baseUrl: "https://generativelanguage.googleapis.com", icon: GeminiIcon },
  { id: "grok", label: "Grok", description: "xAI OAuth、Token 或 API Key", baseUrl: "https://api.x.ai/v1", icon: GrokIcon },
  { id: "antigravity", label: "Antigravity", description: "Google OAuth 或 Token", baseUrl: "https://cloudcode-pa.googleapis.com", icon: AntigravityIcon },
  { id: "kimi", label: "Kimi", description: "Moonshot API Key", baseUrl: "https://api.moonshot.cn/v1", icon: KimiIcon },
  { id: "zhipu", label: "智谱 GLM", description: "智谱开放平台 API Key", baseUrl: "https://open.bigmodel.cn/api/paas/v4", icon: GlmIcon },
  { id: "deepseek", label: "DeepSeek", description: "DeepSeek API Key", baseUrl: "https://api.deepseek.com", icon: DeepSeekIcon },
  { id: "custom", label: "自定义", description: "OpenAI 兼容接口", baseUrl: "", icon: CustomIcon },
] as const

// 托管供应商的地址不参与路由，后端只存一个与部署无关的标记值；
// 界面统一显示成人话，不把标记值暴露给用户。
const MANAGED_POOL_LABEL = "由内置账户池管理"
const emptyForm = { name: "", provider_type: "openai-compatible", base_url: "", api_key: "", avatar_url: "" }
const oauthCallbackMessage = "junohub:oauth-callback"
const oauthPlatforms = new Set(["openai", "anthropic", "gemini", "grok", "antigravity"])
const automaticCallbackOrigins = new Set(["http://localhost:1455", "http://localhost:8085", "http://127.0.0.1:56121"])

const geminiAccountTypes = [
  { value: "code_assist", label: "Gemini Code Assist" },
  { value: "google_one", label: "Google One / Gemini 订阅" },
  { value: "ai_studio", label: "AI Studio 自有 OAuth Client" },
]

export default function ProvidersPage() {
  const isUser = useConsoleGuard()
  const [choosingPlatform, setChoosingPlatform] = useState(true)
  const [routingInvalid, setRoutingInvalid] = useState(false)
  const [routingDraft, setRoutingDraft] = useState<RoutingDraft>({})
  const [policyRevision, setPolicyRevision] = useState(0)
  const [savedPolicy, setSavedPolicy] = useState<DispatchPolicy | null>(null)
  const [capacityBusy, setCapacityBusy] = useState(false)
  const [capacityRevision, setCapacityRevision] = useState(0)
  const [capacityDirty, setCapacityDirty] = useState(false)
  const [pendingLeave, setPendingLeave] = useState<(() => void) | null>(null)
  const [query, setQuery] = useState("")
  const [providers, setProviders] = useState<ConsoleProvider[]>([])
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState("")
  const [formError, setFormError] = useState("")
  const [submitting, setSubmitting] = useState(false)
  const [avatarProcessing, setAvatarProcessing] = useState(false)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editTarget, setEditTarget] = useState<ConsoleProvider | null>(null)
  const [testReport, setTestReport] = useState<{ provider: ConsoleProvider; report: ConnectionTestResult } | null>(null)
  const [testModelOpen, setTestModelOpen] = useState(false)
  const [testSettings, setTestSettings] = useState(() => defaultModelTestSettings("custom"))
  const [testModel, setTestModel] = useState("")
  const [testingId, setTestingId] = useState<number | null>(null)
  const [accountParent, setAccountParent] = useState<ConsoleProvider | null>(null)
  const [managedTab, setManagedTab] = useState<"accounts" | "capacity" | "policy">("accounts")
  const [policy, setPolicy] = useState<DispatchPolicy>({ strategy: "balanced", max_attempts: 3, sticky_sessions: true, request_timeout_seconds: 120, failure_cooldown_seconds: 30 })
  const managedRequest = useRef(0)
  const [managedReady, setManagedReady] = useState(false)
  const [managedReauth, setManagedReauth] = useState(false)
  const [managedTarget, setManagedTarget] = useState<ConsoleProvider | null>(null)
  const [managedAccounts, setManagedAccounts] = useState<ConsoleProviderAccount[]>([])
  const [managedLoading, setManagedLoading] = useState(false)
  const [managedSaving, setManagedSaving] = useState(false)
  const [managedError, setManagedError] = useState("")
  const [managedEditingAccount, setManagedEditingAccount] = useState<ConsoleProviderAccount | null>(null)
  const [managedAccountName, setManagedAccountName] = useState("")
  const [managedAccountBaseUrl, setManagedAccountBaseUrl] = useState("")
  const [managedAccountKey, setManagedAccountKey] = useState("")
  const [managedOauthStart, setManagedOauthStart] = useState<{ auth_url: string; session_id: string; state: string } | null>(null)
  const [managedOauthCode, setManagedOauthCode] = useState("")
  const [managedOauthStarting, setManagedOauthStarting] = useState(false)
  const [managedOauthType, setManagedOauthType] = useState("code_assist")
  const [managedOauthProjectId, setManagedOauthProjectId] = useState("")
  const [managedOauthClientId, setManagedOauthClientId] = useState("")
  const [managedOauthClientSecret, setManagedOauthClientSecret] = useState("")
  const [accountDeleteTarget, setAccountDeleteTarget] = useState<ConsoleProviderAccount | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<{ id: number; name: string } | null>(null)
  const [form, setForm] = useState(emptyForm)
  const [accountOpen, setAccountOpen] = useState(false)
  const [accountPlatform, setAccountPlatform] = useState<(typeof accountPlatforms)[number]["id"]>("openai")
  const [accountName, setAccountName] = useState("OpenAI")
  const [accountKey, setAccountKey] = useState("")
  const [accountBaseUrl, setAccountBaseUrl] = useState("https://api.openai.com/v1")
  const [accountSubmitting, setAccountSubmitting] = useState(false)
  const [accountError, setAccountError] = useState("")
  const [accountMethod, setAccountMethod] = useState<"oauth" | "token" | "api_key">("oauth")
  const [oauthStarting, setOauthStarting] = useState(false)
  const oauthStartLock = useRef(false)
  const managedOauthStartLock = useRef(false)
  const accountSubmitLock = useRef(false)
  const managedSubmitLock = useRef(false)
  const [oauthStart, setOauthStart] = useState<{ auth_url: string; session_id: string; state: string } | null>(null)
  const [oauthCode, setOauthCode] = useState("")
  const [oauthType, setOauthType] = useState("code_assist")
  const [oauthProjectId, setOauthProjectId] = useState("")
  const [oauthClientId, setOauthClientId] = useState("")
  const [oauthClientSecret, setOauthClientSecret] = useState("")
  const [oauthToken, setOauthToken] = useState("")
  const [oauthTokenKind, setOauthTokenKind] = useState<"refresh" | "access">("refresh")

  const limits = useRequestLimits("provider", editTarget?.id ?? null, (dialogOpen || accountOpen) && (Boolean(editTarget) || (!choosingPlatform && accountPlatform === "custom" && !accountParent)))

  const load = async () => {
    setLoading(true); setLoadError("")
    try { const data = await consoleAPI.providerList(); const list = data.list || []; setProviders(list); return list }
    catch (error) { const message = error instanceof Error ? error.message : "无法加载供应商，请检查网络后重试。"; setLoadError(message); toast.error(message); return [] }
    finally { setLoading(false) }
  }

  useEffect(() => { if (isUser) load() }, [isUser])

  useEffect(() => {
    const receiveOAuthCallback = (event: MessageEvent) => {
      if (!automaticCallbackOrigins.has(event.origin) || ![oauthCallbackMessage, "junohub:openai-oauth-callback"].includes(event.data?.type) || !oauthStart) return
      const callbackUrl = typeof event.data.callbackUrl === "string" ? event.data.callbackUrl : ""
      const parsed = parseOAuthResult(callbackUrl, "")
      if (!parsed.code || parsed.state !== oauthStart.state) {
        setAccountError("OAuth 回调校验失败，请重新生成授权链接。")
        return
      }
      setOauthCode(callbackUrl)
      setAccountError("")
      toast.success(`已收到 ${accountPlatforms.find(item => item.id === accountPlatform)?.label || accountPlatform} 授权结果`)
    }
    window.addEventListener("message", receiveOAuthCallback)
    return () => window.removeEventListener("message", receiveOAuthCallback)
  }, [oauthStart, accountPlatform])

  useEffect(() => {
    const receiveManagedOAuthCallback = (event: MessageEvent) => {
      if (!managedOauthStart || !managedEditingAccount || !automaticCallbackOrigins.has(event.origin) || ![oauthCallbackMessage, "junohub:openai-oauth-callback"].includes(event.data?.type)) return
      const callbackUrl = typeof event.data.callbackUrl === "string" ? event.data.callbackUrl : ""
      const parsed = parseOAuthResult(callbackUrl, "")
      if (!parsed.code || parsed.state !== managedOauthStart.state) {
        setManagedError("OAuth 回调校验失败，请重新生成授权链接。")
        return
      }
      setManagedOauthCode(callbackUrl)
      setManagedError("")
      toast.success("已收到新的授权结果")
    }
    window.addEventListener("message", receiveManagedOAuthCallback)
    return () => window.removeEventListener("message", receiveManagedOAuthCallback)
  }, [managedOauthStart, managedEditingAccount])

  const openAdd = () => {
    setChoosingPlatform(true)
    setAccountParent(null)
    const platform = accountPlatforms[0]
    setEditTarget(null); setForm(emptyForm); setFormError(""); setAccountPlatform(platform.id); setAccountName(platform.label); setAccountKey(""); setAccountBaseUrl(platform.baseUrl); setAccountMethod("oauth"); resetOAuthFields(); setAccountError(""); setAccountOpen(true)
  }

  const selectAccountPlatform = (platform: typeof accountPlatforms[number]) => {
    setChoosingPlatform(false)
    setAccountKey("")
    setAccountPlatform(platform.id); setAccountName(platform.label); setAccountBaseUrl(platform.baseUrl); setAccountMethod(oauthPlatforms.has(platform.id) ? "oauth" : "api_key"); resetOAuthFields(); setAccountError("")
    if (platform.id === "custom") { setForm(emptyForm); setFormError("") }
  }

  const resetOAuthFields = () => { setOauthStart(null); setOauthCode(""); setOauthType("code_assist"); setOauthProjectId(""); setOauthClientId(""); setOauthClientSecret(""); setOauthToken(""); setOauthTokenKind("refresh") }

  const startOAuth = async () => {
    if (oauthStartLock.current || accountSubmitLock.current) return
    oauthStartLock.current = true
    const popup = window.open("about:blank", `${accountPlatform}-oauth`, "width=720,height=780")
    setOauthStarting(true); setOauthStart(null); setOauthCode(""); setAccountError("")
    try {
      const result = accountPlatform === "openai"
        ? await consoleAPI.startOpenAIOAuth()
        : await consoleAPI.startProviderOAuth(accountPlatform, { oauth_type: oauthType, project_id: oauthProjectId.trim(), client_id: oauthClientId.trim(), client_secret: oauthClientSecret })
      setOauthStart(result)
      if (popup) popup.location.href = result.auth_url
      else window.open(result.auth_url, "_blank", "noopener,noreferrer")
    } catch (error) {
      popup?.close()
      setAccountError(error instanceof Error ? error.message : "无法开始账户授权。")
    } finally { oauthStartLock.current = false; setOauthStarting(false) }
  }

  const createAccount = async (event: React.FormEvent) => {
    event.preventDefault()
    if (accountPlatform === "custom" && !accountParent) return
    if (accountParent && managedAccounts.some(a => a.name.trim().toLowerCase() === accountName.trim().toLowerCase())) { setAccountError("此供应商下已有同名账户，请换一个名称。"); return }
    if (accountSubmitLock.current || oauthStartLock.current) return
    accountSubmitLock.current = true
    setAccountSubmitting(true); setAccountError("")
    try {
      if (accountParent && !accountParent.managed) {
        await consoleAPI.providerAccountCreate(accountParent.id, { name: accountName.trim(), api_key: accountKey.trim() })
      } else if (accountMethod === "oauth") {
        if (!oauthStart) { setAccountError("请先打开授权页面。"); return }
        const parsed = parseOAuthResult(oauthCode, oauthStart.state)
        if (!parsed.code) { setAccountError(parsed.error || "请输入授权码或完整回调地址。"); return }
        await (accountPlatform === "openai"
          ? consoleAPI.completeOpenAIOAuth({ session_id: oauthStart.session_id, state: parsed.state, code: parsed.code, target_provider_id: accountParent?.id, name: accountName.trim() })
          : consoleAPI.completeProviderOAuth(accountPlatform, { session_id: oauthStart.session_id, state: parsed.state, code: parsed.code, target_provider_id: accountParent?.id, name: accountName.trim() }))
      } else if (accountMethod === "token") {
        if (!oauthToken.trim()) { setAccountError("请输入 Refresh Token 或 Access Token。"); return }
        await consoleAPI.importProviderOAuthToken(accountPlatform, { target_provider_id: accountParent?.id, name: accountName.trim(), ...(oauthTokenKind === "access" ? { access_token: oauthToken.trim() } : { refresh_token: oauthToken.trim() }), oauth_type: oauthType, project_id: oauthProjectId.trim(), client_id: oauthClientId.trim(), client_secret: oauthClientSecret })
      } else {
        if (!accountKey.trim()) { setAccountError("请输入 API Key。"); return }
        if (!accountBaseUrl.trim()) { setAccountError("请输入 Base URL。"); return }
        await consoleAPI.createSub2apiApiKeyAccount({ platform: accountPlatform, target_provider_id: accountParent?.id, name: accountName.trim(), api_key: accountKey.trim(), base_url: accountBaseUrl.trim() })
      }
      setAccountOpen(false)
      await load()
      if (accountParent) { try { const data = await consoleAPI.providerAccounts(accountParent.id); setManagedAccounts(data.list || []) } catch { setManagedError("授权已添加，但账户列表刷新失败。请刷新状态。") } setAccountParent(null) }
      resetOAuthFields()
      toast.success(`${selectedAccountLabel} 账户已添加`)
    } catch (error) { setAccountError(error instanceof Error ? error.message : "无法接入账户，请检查凭证。") }
    finally { accountSubmitLock.current = false; setAccountSubmitting(false) }
  }

  const openEdit = async (item: ConsoleProvider) => {
    if (item.managed || item.provider_type !== "sub2api") {
      const requestId = ++managedRequest.current
      setRoutingDraft({})
      setManagedReady(false)
      setCapacityDirty(false)
      setSavedPolicy(null)
      setManagedTab("accounts"); setManagedTarget(item); setManagedAccounts([]); setManagedEditingAccount(null); setManagedError(""); setManagedLoading(true)
      try {
        const [data, savedPolicy] = await Promise.all([consoleAPI.providerAccounts(item.id), consoleAPI.providerPolicy(item.id)])
        if (requestId !== managedRequest.current) return
        setManagedReady(true)
        setPolicy(savedPolicy)
        setSavedPolicy(savedPolicy)
        const accounts = data.list || []
        setManagedAccounts(accounts)
      } catch (error) { if (requestId === managedRequest.current) setManagedError(error instanceof Error ? error.message : "无法加载供应商账户。") }
      finally { if (requestId === managedRequest.current) setManagedLoading(false) }
      return
    }
    setEditTarget(item)
    setForm({ name: item.name, provider_type: item.provider_type, base_url: item.base_url, api_key: "", avatar_url: item.avatar_url || "" })
    setFormError(""); setDialogOpen(true)
  }

  const withCleanCapacity = (action: () => void) => {
    if (capacityBusy) return
    if (capacityDirty) setPendingLeave(() => () => { setCapacityRevision(n => n + 1); setCapacityDirty(false); action() })
    else action()
  }

  const addManagedAuthorization = () => withCleanCapacity(() => {
    if (!managedTarget) return
    const platform = accountPlatforms.find(item => item.id === (managedTarget.managed ? managedTarget.platform : "custom"))
    if (!platform) { setManagedError("无法识别供应商平台。"); return }
    openAdd()
    selectAccountPlatform(platform)
    setAccountName(`${platform.label} ${managedTarget.managed ? "账户" : "Key"} ${managedAccounts.length + 1}`)
    if (!managedTarget.managed) { setAccountMethod("api_key"); setAccountBaseUrl(managedTarget.base_url) }
    setAccountParent(managedTarget)
  })

  const savePolicy = async (event: React.FormEvent) => {
    event.preventDefault()
    if (!managedTarget || !managedReady || routingInvalid) return
    setManagedSaving(true); setManagedError("")
    let accountsSaved = false
    try {
      const updated = managedAccounts.map(a => ({ ...a, ...routingDraft[a.id] }))
      if (updated.some(a => (a.weight ?? 1) < 1 || (a.weight ?? 1) > 1000)) { setManagedError("每个账户的份额请填写 1–1000。"); return }
      if (routingDirty) {
        await consoleAPI.providerAccountDispatch(managedTarget.id, updated.map(a => ({ id: a.id, priority: a.priority, member_order: a.member_order ?? 0, weight: a.weight ?? 1, concurrency: a.concurrency ?? 1, rpm: a.rpm || 0, allowed_models: a.allowed_models || [] })))
        setManagedAccounts(updated); setRoutingDraft({}); accountsSaved = true
      }
      const saved = await consoleAPI.providerPolicyUpdate(managedTarget.id, policy)
      setPolicy(saved); setSavedPolicy(saved); toast.success("请求分配已保存")
    }
    catch (error) { setManagedError(`${accountsSaved ? "账户顺序和份额已保存，但分配方式未保存。请重试保存。" : ""}${error instanceof Error ? error.message : "无法保存请求分配。"}`) }
    finally { setManagedSaving(false) }
  }

  const openManagedAccountEdit = (account: ConsoleProviderAccount, credentials = false) => withCleanCapacity(() => {
    setManagedReauth(credentials)
    setManagedEditingAccount(account)
    setManagedAccountName(account.name)
    setManagedAccountBaseUrl(account.base_url || "")
    setManagedAccountKey("")
    setManagedOauthStart(null)
    setManagedOauthCode("")
    setManagedOauthType(account.oauth_type || "code_assist")
    setManagedOauthProjectId(account.project_id || "")
    setManagedOauthClientId(account.client_id || "")
    setManagedOauthClientSecret("")
    setManagedError("")
  })

  const startManagedOAuth = async () => {
    if (!managedEditingAccount || managedOauthStartLock.current || managedSubmitLock.current) return
    const platform = managedEditingAccount.platform
    if (platform === "gemini" && managedOauthType === "ai_studio" && (!managedOauthClientId.trim() || !managedOauthClientSecret)) {
      setManagedError("重新授权 AI Studio 账户时，请填写 OAuth Client ID 和 Client Secret。")
      return
    }
    managedOauthStartLock.current = true
    const popup = window.open("about:blank", `${platform}-oauth-edit`, "width=720,height=780")
    setManagedOauthStart(null); setManagedOauthStarting(true); setManagedOauthCode(""); setManagedError("")
    try {
      const result = platform === "openai"
        ? await consoleAPI.startOpenAIOAuth()
        : await consoleAPI.startProviderOAuth(platform, { oauth_type: managedOauthType, project_id: managedOauthProjectId.trim(), client_id: managedOauthClientId.trim(), client_secret: managedOauthClientSecret })
      setManagedOauthStart(result)
      if (popup) popup.location.href = result.auth_url
      else window.open(result.auth_url, "_blank", "noopener,noreferrer")
    } catch (error) {
      popup?.close()
      setManagedError(error instanceof Error ? error.message : "无法开始重新授权。")
    } finally { managedOauthStartLock.current = false; setManagedOauthStarting(false) }
  }

  const saveManagedAccount = async (event: React.FormEvent) => {
    event.preventDefault()
    if (!managedTarget || !managedEditingAccount) return
    if (!managedAccountName.trim()) { setManagedError("请输入账户名称。"); return }
    if (managedAccounts.some(a => a.id !== managedEditingAccount.id && a.name.trim().toLowerCase() === managedAccountName.trim().toLowerCase())) { setManagedError("此供应商下已有同名账户，请换一个名称。"); return }
    if (managedTarget.managed && !managedReauth && managedEditingAccount.kind === "apikey" && !managedAccountBaseUrl.trim()) { setManagedError("请输入 Base URL。"); return }
    if (managedSubmitLock.current || managedOauthStartLock.current) return
    managedSubmitLock.current = true
    setManagedSaving(true); setManagedError("")
    try {
      if (managedReauth && managedEditingAccount.kind === "oauth") {
        if (!managedOauthStart) { setManagedError("请先完成授权。"); return }
        const parsed = parseOAuthResult(managedOauthCode, managedOauthStart.state)
        if (!parsed.code) { setManagedError(parsed.error || "请完成授权，或粘贴授权码或完整回调地址。"); return }
        const oauthUpdate = { session_id: managedOauthStart.session_id, state: parsed.state, code: parsed.code, name: managedAccountName.trim(), provider_id: managedTarget.id, account_id: managedEditingAccount.id }
        await (managedEditingAccount.platform === "openai"
          ? consoleAPI.completeOpenAIOAuth(oauthUpdate)
          : consoleAPI.completeProviderOAuth(managedEditingAccount.platform, oauthUpdate))
      } else {
        await consoleAPI.providerAccountUpdate(managedTarget.id, {
          account_id: managedEditingAccount.id,
          name: managedAccountName.trim(),
          ...(managedEditingAccount.kind === "apikey" ? { ...(managedTarget.managed ? { base_url: managedAccountBaseUrl.trim() } : {}), ...(managedReauth && managedAccountKey.trim() ? { api_key: managedAccountKey.trim() } : {}) } : {}),
        })
      }
      setManagedAccounts(current => current.map(account => account.id === managedEditingAccount.id ? { ...account, name: managedAccountName.trim(), base_url: !managedTarget.managed ? managedTarget.base_url : managedEditingAccount.kind === "apikey" ? managedAccountBaseUrl.trim().replace(/\/$/, "") : account.base_url } : account))
      setManagedEditingAccount(null)
      toast.success(managedReauth ? "账户凭证已更新" : "账户资料已保存")
      try { const data = await consoleAPI.providerAccounts(managedTarget.id); setManagedAccounts(data.list || []) } catch { toast.error("已保存，但状态刷新失败，请手动刷新。") }
    } catch (error) { setManagedError(error instanceof Error ? error.message : "无法保存账户配置。") }
    finally { managedSubmitLock.current = false; setManagedSaving(false) }
  }
  const editProviderConnection = (item: ConsoleProvider, suggestedUrl?: string) => {
    setTestReport(null); setAccountParent(null); setAccountOpen(false)
    setEditTarget(item)
    setForm({ name: item.name, provider_type: item.provider_type, base_url: suggestedUrl || item.base_url, api_key: "", avatar_url: item.avatar_url || "" })
    setFormError(""); setDialogOpen(true)
  }

  const saveProvider = async (event: React.FormEvent) => {
    event.preventDefault()
    if (avatarProcessing) { setFormError("请等待头像图片处理完成。"); return }
    setFormError(""); setSubmitting(true)
    try {
      if (editTarget) {
        const update = { request_limits: limits.value(), id: editTarget.id, name: form.name.trim(), provider_type: form.provider_type, base_url: form.base_url.trim(), avatar_url: form.avatar_url, ...(form.api_key ? { api_key: form.api_key } : {}) }
        const saved = await consoleAPI.providerUpdate(update)
        if (update.avatar_url ? !saved.avatar_url : saved.avatar_url) {
          throw new Error("头像未保存成功，请稍后重试。")
        }
        if (managedTarget?.id === editTarget.id) {
          const base_url = update.base_url.replace(/\/$/, "")
          setManagedTarget(current => current ? { ...current, name: update.name, base_url, avatar_url: saved.avatar_url || "" } : current)
          setManagedAccounts(current => current.map(account => ({ ...account, base_url })))
        }
        setDialogOpen(false); await load(); toast.success("供应商配置已更新")
      } else {
        const created = await consoleAPI.providerCreate({ request_limits: limits.value(), ...form, name: form.name.trim(), base_url: form.base_url.trim() })
        if (form.avatar_url && !created.avatar_url) {
          throw new Error("头像未保存成功，请稍后重试。")
        }
        setAccountOpen(false); setDialogOpen(false); await load(); toast.success("供应商已添加")
      }
    } catch (error) { setFormError(error instanceof Error ? error.message : "无法保存供应商，请检查填写内容后重试。") }
    finally { setSubmitting(false) }
  }

  const testProvider = async (item: ConsoleProvider, model?: string) => {
    const displayName = providerPresentation(item).title
    setTestingId(item.id)
    setTestReport({ provider: item, report: { ok: false, outcome: "warning", status_code: 0, message: "" } })
    if (!model) { setTestModel(""); setTestModelOpen(false); setTestSettings(defaultModelTestSettings(item.provider_type)) }
    try {
      const result = await consoleAPI.providerTest(item.id, model, item.provider_type === "gemini" ? undefined : testSettings)
      setTestReport({ provider: item, report: result })
      if (!model) setTestModel("")
    } catch (error) { setTestReport({ provider: item, report: { ok: false, outcome: "error", status_code: 0, message: error instanceof Error ? error.message : `无法测试 ${displayName}。` } }) }
    finally { setTestingId(null) }
  }

  const oauthResult = oauthStart ? parseOAuthResult(oauthCode, oauthStart.state) : null
  const managedOauthResult = managedOauthStart ? parseOAuthResult(managedOauthCode, managedOauthStart.state) : null
  const oauthCodeReady = Boolean(oauthResult?.code)
  const managedOauthCodeReady = Boolean(managedOauthResult?.code)
  const selectedAccountLabel = accountPlatforms.find(item => item.id === accountPlatform)?.label || accountPlatform

  const routingAccounts = managedAccounts.map(a => ({ ...a, ...routingDraft[a.id] }))
  const routingDirty = managedAccounts.some(a => routingDraft[a.id] && (routingDraft[a.id].priority !== a.priority || routingDraft[a.id].weight !== a.weight || routingDraft[a.id].member_order !== (a.member_order ?? 0)))
  const policyDirty = routingInvalid || routingDirty || Boolean(savedPolicy && JSON.stringify(policy) !== JSON.stringify(savedPolicy))
  const accountDirty = Boolean(managedEditingAccount && (managedAccountName !== managedEditingAccount.name || managedAccountBaseUrl !== (managedEditingAccount.base_url || "") || managedAccountKey || managedOauthStart || managedOauthCode))
  const leaveAccount = () => {
    const close = () => { setManagedEditingAccount(null); setManagedOauthStart(null); setManagedOauthCode(""); setManagedError("") }
    if (managedSaving || managedOauthStarting) return
    if (accountDirty) setPendingLeave(() => close); else close()
  }
  const leaveWorkspace = () => {
    const close = () => { managedRequest.current++; setManagedTarget(null); setManagedEditingAccount(null); setCapacityDirty(false) }
    if (managedSaving || capacityBusy) return
    if (policyDirty || capacityDirty) setPendingLeave(() => close); else close()
  }

  const search = query.trim().toLocaleLowerCase()
  const visibleItems = providers.filter(item => [providerPresentation(item).title, providerPresentation(item).meta].join(" ").toLocaleLowerCase().includes(search))

  if (loading && !providers.length) return <LoadingState />
  if (loadError && !providers.length) return <ErrorState message={loadError} onRetry={load} />

  return (
    <div className="view-enter space-y-8">
      <div hidden={Boolean(managedTarget)} className="space-y-8"><PageHead title="供应商" description="按平台管理授权账户，为模型路由提供上游连接。" />

      <section className="space-y-4" aria-label="供应商列表">
      {providers.length > 0 && (
        <div className="flex flex-col gap-3 border-b border-border-default pb-4 xl:flex-row xl:items-center xl:justify-between">
          <p className="text-sm font-medium text-foreground tabular-nums" role="status">{query.trim() ? `找到 ${visibleItems.length} / ${providers.length} 个供应商` : `共 ${providers.length} 个供应商`}</p>
          <div className="flex min-w-0 flex-col gap-3 min-[480px]:flex-row min-[480px]:items-center min-[480px]:justify-end">
            <div className="w-full min-w-0 min-[480px]:w-80">
              <InputGroup>
                <InputGroupInput aria-label="搜索供应商" placeholder="搜索供应商" type="search" value={query} onChange={event => setQuery(event.target.value)} />
                <InputGroupAddon>
                  <SearchIcon aria-hidden="true" />
                </InputGroupAddon>
              </InputGroup>
            </div>
            <CossButton onClick={openAdd}>添加供应商</CossButton>
          </div>
        </div>
      )}

      {providers.length === 0 ? (
        <EmptyGrid title="还没有供应商" description="添加一个上游账户，统一管理连接与凭证。" actionLabel="添加供应商" onAction={openAdd} action={<CossButton onClick={openAdd}>添加供应商</CossButton>} />
      ) : visibleItems.length === 0 ? (
        <NoSearchResults onClear={() => setQuery("")} />
      ) : (
        <ResourceGrid>
          {visibleItems.map(item => {
            const presentation = providerPresentation(item)
            return (
              <ResourceCard
                key={item.id}
                badge={<ProviderAvatar provider={item} />}
                title={presentation.title}
                meta={presentation.meta}
                status={{ active: item.enabled === 1, label: item.enabled === 1 ? "已启用" : "已停用" }}
                actions={<div className="flex items-center gap-1"><Button size="sm" variant="default" onClick={() => openEdit(item)} aria-label={`配置 ${presentation.title}`} title={item.managed ? "管理授权与策略" : "编辑配置"}><Settings2 aria-hidden="true" />{(item.managed || item.provider_type !== "sub2api") ? "管理供应商" : "编辑连接"}</Button><Button size="sm" variant="outline" onClick={() => testProvider(item)} disabled={testingId === item.id}>{testingId === item.id && <RotateCw className="animate-spin" aria-hidden="true" />}测试连接</Button></div>}
                onDelete={() => setDeleteTarget({ id: item.id, name: presentation.title })}
                deleteLabel={`删除 ${presentation.title}`}
              />
            )
          })}
        </ResourceGrid>
      )}

      </section></div>

      <FormDialog open={accountOpen || dialogOpen} onOpenChange={open => { if (!open && !accountSubmitting && !submitting && !avatarProcessing && !oauthStarting) { setAccountOpen(false); setDialogOpen(false); resetOAuthFields() } }} title={accountParent ? `为 ${providerPresentation(accountParent).title} 添加${accountParent.managed ? "授权" : " Key"}` : editTarget ? "编辑供应商" : "添加供应商"} description={accountParent ? (accountParent.managed ? "使用另一个账户授权，或添加独立的 API Key。" : "新增独立 Key，加入当前供应商的调度与失败切换。") : editTarget ? "修改供应商名称和 Base URL，地址由该供应商下的所有账户共用。" : "选择平台和认证方式，完成供应商连接配置。"} contentClassName={editTarget ? "sm:max-w-xl" : "sm:max-w-4xl"}>
        {!editTarget && !accountParent && choosingPlatform && <fieldset className="mb-6"><legend className="text-sm font-semibold">选择要接入的平台</legend><p className="mt-2 text-xs leading-5 text-muted-foreground">已接入的平台进入现有供应商添加账户；新平台完成首次授权后创建供应商。</p><div className="mt-3 grid grid-cols-1 gap-2 min-[480px]:grid-cols-2 md:grid-cols-3">{accountPlatforms.map(platform => <PlatformButton key={platform.id} platform={{ ...platform, description: providers.some(item => item.managed && item.platform === platform.id) ? "已接入 · 进入管理添加账户" : platform.description }} selected={false} onClick={() => { const existing = providers.find(item => item.managed && item.platform === platform.id); if (existing) { setAccountOpen(false); void openEdit(existing) } else selectAccountPlatform(platform) }} />)}</div></fieldset>}
        {!editTarget && !accountParent && !choosingPlatform && <Button type="button" variant="ghost" size="sm" className="mb-4" disabled={accountSubmitting || oauthStarting} onClick={() => { setChoosingPlatform(true); resetOAuthFields() }}><ChevronLeft aria-hidden="true" />重新选择平台 · {selectedAccountLabel}</Button>}
        {(!choosingPlatform || editTarget || accountParent) && (!editTarget && (accountPlatform !== "custom" || accountParent) ? (
        <AppForm onSubmit={createAccount} className="mx-auto max-w-2xl space-y-5 rounded-2xl bg-[var(--glass-bg-subtle)] p-4 ring-1 ring-inset ring-[var(--glass-border-subtle)] sm:p-5" aria-busy={accountSubmitting}>
          <fieldset disabled={accountSubmitting || oauthStarting}><legend className="text-sm font-semibold">认证方式</legend><div className="mt-3 grid grid-cols-[repeat(auto-fit,minmax(10rem,1fr))] gap-2">
            {oauthPlatforms.has(accountPlatform) && <AuthMethodButton selected={accountMethod === "oauth"} title="浏览器 OAuth" description="登录订阅账户授权" onClick={() => { setAccountMethod("oauth"); setAccountError("") }} />}
            {accountPlatform !== "openai" && oauthPlatforms.has(accountPlatform) && <AuthMethodButton selected={accountMethod === "token"} title="导入 Token" description="已有凭证直接接入" onClick={() => { setAccountMethod("token"); setAccountError("") }} />}
            {accountPlatform !== "antigravity" && <AuthMethodButton selected={accountMethod === "api_key"} title="API Key" description="使用平台开发者密钥" onClick={() => { setAccountMethod("api_key"); setAccountError("") }} />}
          </div></fieldset>
          <Field id="account-name" label="账户名称"><Input id="account-name" value={accountName} onChange={event => setAccountName(event.target.value)} placeholder="例如：我的 OpenAI" required /></Field>
          {accountPlatform === "gemini" && accountMethod !== "api_key" && <div className="space-y-3">
            <Field id="oauth-type" label="Gemini 账户类型"><SelectField id="oauth-type" value={oauthType} onValueChange={value => { setOauthType(value); setOauthStart(null); setOauthCode("") }} options={geminiAccountTypes} /></Field>
            <Field id="oauth-project" label="Google Cloud Project ID" hint="Code Assist / Google One 建议填写，留空会尝试自动识别"><Input id="oauth-project" value={oauthProjectId} onChange={event => setOauthProjectId(event.target.value)} placeholder="my-google-cloud-project" /></Field>
            {oauthType === "ai_studio" && <><Field id="oauth-client-id" label="OAuth Client ID"><Input id="oauth-client-id" value={oauthClientId} onChange={event => setOauthClientId(event.target.value)} required /></Field><Field id="oauth-client-secret" label="OAuth Client Secret"><Input id="oauth-client-secret" type="password" value={oauthClientSecret} onChange={event => setOauthClientSecret(event.target.value)} required /></Field></>}
          </div>}
          {accountPlatform === "antigravity" && accountMethod !== "api_key" && <Field id="oauth-project" label="Google Cloud Project ID" hint="留空会尝试自动识别；识别失败时需要重新绑定并填写"><Input id="oauth-project" value={oauthProjectId} onChange={event => setOauthProjectId(event.target.value)} placeholder="my-google-cloud-project" /></Field>}
          {accountMethod === "oauth" ? <div className="space-y-3">
            <OAuthStep number={1} title={`打开 ${selectedAccountLabel} 授权页`} complete={Boolean(oauthStart)}>
              <p className="text-xs leading-5 text-muted-foreground">从这里打开授权页，登录并同意授权，再将授权结果复制回来。请勿使用官方客户端登录生成的回调地址。</p>
              <Button type="button" variant="outline" className="mt-3 w-full" onClick={startOAuth} disabled={oauthStarting || accountSubmitting}>{oauthStarting && <RotateCw className="animate-spin" aria-hidden="true" />}{oauthStart ? "重新生成并打开授权页" : "生成并打开授权页"}<ExternalLink aria-hidden="true" /></Button>
            </OAuthStep>
            {oauthStart && <>
              <p className="text-xs leading-5 text-muted-foreground">需要继续登录时，请打开本次授权页。重新生成会切换会话，旧回调将不能在此提交。</p>
              <OAuthStep number={2} title="复制授权结果" complete={oauthCodeReady}>
                <p className="text-xs leading-5 text-muted-foreground">如果平台显示授权码，请复制授权码；如果跳转到 localhost，请复制地址栏中包含 code 和 state 的完整地址。即使回调页面无法连接，也可以复制地址并继续。</p>
                <a href={oauthStart.auth_url} target="_blank" rel="noreferrer" className="mt-3 inline-flex items-center gap-2 text-sm font-medium text-accent-text hover:underline">打开本次授权页<ExternalLink className="size-4" strokeWidth={1.75} aria-hidden="true" /></a>
              </OAuthStep>
              <OAuthStep number={3} title="粘贴地址并完成授权" complete={false}>
                <Field id="oauth-result" label="回调地址或授权码" hint="粘贴本次授权的完整回调地址或授权码，也支持 Claude 的 code#state"><Input id="oauth-result" value={oauthCode} onChange={event => { setOauthCode(event.target.value); setAccountError("") }} placeholder="粘贴授权结果" autoComplete="off" spellCheck={false} /></Field>
                <FormError message={oauthResult?.error || ""} />
                {oauthCodeReady && <p className="mt-2 flex items-center gap-1.5 text-xs font-medium text-success-text"><Check className="size-3.5" strokeWidth={2} aria-hidden="true" />已读取授权码，提交后验证账户</p>}
              </OAuthStep>
            </>}
          </div> : accountMethod === "token" ? <><div className="grid grid-cols-2 gap-2"><AuthMethodButton selected={oauthTokenKind === "refresh"} title="Refresh Token" description="可自动续期，建议使用" onClick={() => setOauthTokenKind("refresh")} /><AuthMethodButton selected={oauthTokenKind === "access"} title="Access Token" description="到期后需要重新导入" onClick={() => setOauthTokenKind("access")} /></div><Field id="oauth-token" label={oauthTokenKind === "refresh" ? "Refresh Token" : "Access Token"}><Input id="oauth-token" type="password" autoComplete="off" spellCheck={false} value={oauthToken} onChange={event => { setOauthToken(event.target.value); setAccountError("") }} placeholder="粘贴 Token" required /></Field></> : <><Field id="account-api-key" label="API Key"><Input id="account-api-key" type="password" autoComplete="off" spellCheck={false} value={accountKey} onChange={event => { setAccountKey(event.target.value); setAccountError("") }} placeholder="输入平台 API Key" required /></Field>{accountParent && !accountParent.managed ? <SharedProviderAddress url={accountParent.base_url} /> : <Field id="account-base-url" label="Base URL"><Input id="account-base-url" type="url" inputMode="url" spellCheck={false} value={accountBaseUrl} onChange={event => setAccountBaseUrl(event.target.value)} placeholder="https://api.example.com/v1" required /></Field>}</>}
          {accountMethod === "api_key" && <ConnectionTest baseUrl={accountParent && !accountParent.managed ? accountParent.base_url : accountBaseUrl} apiKey={accountKey} platform={accountPlatform} disabled={accountSubmitting} onUseAddress={accountParent && !accountParent.managed ? undefined : setAccountBaseUrl} />}
          <FormError message={accountError} />
          <div className="flex gap-3 border-t border-[var(--glass-border-subtle)] pt-5"><Button type="button" variant="outline" className="flex-1" disabled={accountSubmitting || oauthStarting} onClick={() => { setAccountOpen(false); resetOAuthFields() }}>取消</Button><Button type="submit" className="flex-1" disabled={accountSubmitting || oauthStarting}>{accountSubmitting && <RotateCw className="animate-spin" aria-hidden="true" />}{accountMethod === "oauth" ? "完成授权" : accountParent ? (accountParent.managed ? "添加授权" : "添加 Key") : "添加供应商"}</Button></div>
        </AppForm>
        ) : (
        <AppForm onSubmit={saveProvider} className="mx-auto max-w-2xl space-y-5 rounded-2xl bg-[var(--glass-bg-subtle)] p-4 ring-1 ring-inset ring-[var(--glass-border-subtle)] sm:p-5" aria-busy={submitting}>
          {editTarget && <p className="text-sm text-muted-foreground">连接类型：{providerLabel(editTarget.provider_type)}</p>}
          <Field id="provider-name" label={editTarget ? "供应商名称" : "名称"}><Input id="provider-name" name="provider-name" value={form.name} onChange={event => setForm({ ...form, name: event.target.value })} placeholder={editTarget ? "供应商名称" : "例如：自定义 API"} required /></Field>
          {(form.provider_type === "openai-compatible" || editTarget?.platform === "custom") && <ProviderAvatarEditor avatarUrl={form.avatar_url} onChange={value => setForm(current => ({ ...current, avatar_url: value }))} onProcessingChange={setAvatarProcessing} disabled={submitting} />}
          <Field id="provider-url" label="Base URL" hint={editTarget?.managed ? "由内置账户池管理" : editTarget ? "此地址由该供应商的所有账户共用" : undefined}><Input id="provider-url" name="provider-url" type="url" inputMode="url" spellCheck={false} value={form.base_url} onChange={event => setForm({ ...form, base_url: event.target.value })} placeholder="https://api.example.com/v1" required disabled={editTarget?.managed} /></Field>
          {(!editTarget || (!editTarget.managed && editTarget.provider_type === "sub2api")) && <Field id="provider-key" label={form.provider_type === "sub2api" ? "Sub2API API Key" : "上游 API Key"} hint={editTarget ? "留空保持现有凭证" : form.provider_type === "openai-compatible" ? "可选" : undefined}><Input id="provider-key" name="provider-key" type="password" autoComplete="off" spellCheck={false} value={form.api_key} onChange={event => setForm({ ...form, api_key: event.target.value })} placeholder={editTarget ? "不更改可留空" : "输入上游 API Key"} required={!editTarget && form.provider_type !== "openai-compatible"} /></Field>}
          {!editTarget && <ConnectionTest baseUrl={form.base_url} apiKey={form.api_key} disabled={submitting} onUseAddress={url => setForm(current => ({ ...current, base_url: url }))} />}
          <RequestLimitsFields limits={limits} prefix="provider-edit" disabled={submitting || avatarProcessing} />
          <FormError message={formError} />
          <div className="flex gap-3 border-t border-[var(--glass-border-subtle)] pt-5"><Button type="button" variant="outline" className="flex-1" disabled={submitting || avatarProcessing} onClick={() => { setAccountOpen(false); setDialogOpen(false) }}>取消</Button><Button type="submit" className="flex-1" disabled={submitting || avatarProcessing}>{submitting && <RotateCw className="animate-spin" aria-hidden="true" />}{editTarget ? "保存" : "添加供应商"}</Button></div>
        </AppForm>
        ))}
      </FormDialog>

      {managedTarget && <section className="space-y-6" aria-label="供应商详情">
        <Button type="button" variant="ghost" size="sm" className="-ms-2" onClick={leaveWorkspace} disabled={capacityBusy || managedSaving}><ChevronLeft aria-hidden="true" />返回供应商</Button>
        <PageHead title={providerPresentation(managedTarget).title} description={managedTarget.managed ? "管理已授权的账户，以及它们的使用方式。" : "管理连接密钥，以及请求的分配方式。"} action={<Button onClick={addManagedAuthorization} disabled={!managedReady || managedSaving || capacityBusy}><Plus aria-hidden="true" />{managedTarget.managed ? "添加授权" : "添加 Key"}</Button>} />
        <div className="flex flex-wrap items-center gap-x-5 gap-y-2 text-sm text-muted-foreground"><span>{managedReady ? `${managedAccounts.length} 个${managedTarget.managed ? "授权账户" : "上游 Key"}` : "正在读取配置"}</span><span className="h-3 w-px bg-border-default" aria-hidden="true" /><span>{savedPolicy ? `当前使用${({ priority: "按组内顺序", round_robin: "轮流分配", weighted: "按比例分配", balanced: "空闲优先" })[savedPolicy.strategy]}` : "正在读取分配方式"}</span>{(policyDirty || capacityDirty) && <span className="rounded-full bg-accent-subtle px-2.5 py-1 text-xs font-medium text-accent-text">有修改未保存</span>}</div>
        <div className="space-y-5">
        <nav className="grid grid-cols-1 gap-2 sm:grid-cols-3" aria-label="供应商管理分区">
          {([{ value: "accounts", label: managedTarget.managed ? "授权账户" : "连接密钥", hint: "添加、更新和移除凭证" }, { value: "policy", label: "请求分配", hint: "选择分配方式与失败处理" }, { value: "capacity", label: "账户容量", hint: "设置各账户的并发与请求上限" }] as const).map(tab => <button key={tab.value} type="button" disabled={managedSaving || capacityBusy} className={cn("rounded-xl border px-4 py-3.5 text-start transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:opacity-50", managedTab === tab.value ? "border-accent-text bg-accent-subtle text-accent-text" : "border-border-default bg-bg-surface text-foreground hover:border-border-strong")} aria-pressed={managedTab === tab.value} onClick={() => { setManagedTab(tab.value); setManagedError("") }}><span className="flex items-center justify-between gap-2 text-sm font-semibold">{tab.label}{managedTab === tab.value && <Check className="size-4 shrink-0" aria-hidden="true" />}{((tab.value === "capacity" && capacityDirty) || (tab.value === "policy" && policyDirty)) && <span aria-label="未保存">·</span>}</span><span className="mt-1 block text-xs leading-5 text-muted-foreground">{tab.hint}</span></button>)}
        </nav>
        <div className="min-w-0 rounded-2xl border border-border-default bg-bg-surface p-4 sm:p-6">
        <FormDialog open={Boolean(managedEditingAccount)} onOpenChange={open => { if (!open) leaveAccount() }} title={managedReauth ? "更新账户凭证" : "编辑账户资料"} description={managedEditingAccount ? `正在修改“${managedEditingAccount.name}”，其他账户保持不变。` : ""} contentClassName="sm:max-w-xl">
        {managedEditingAccount && <AppForm onSubmit={saveManagedAccount} className="space-y-5" aria-busy={managedSaving}>
            <fieldset disabled={managedSaving || managedOauthStarting} className="rounded-2xl bg-[var(--glass-bg-subtle)] p-4 ring-1 ring-inset ring-[var(--glass-border-subtle)] sm:p-5">
              <div className="mb-5 flex flex-wrap items-center gap-2"><span className="rounded-full bg-bg-surface px-2.5 py-1 text-xs font-medium text-muted-foreground ring-1 ring-inset ring-[var(--glass-border)]">{accountKindLabel(managedEditingAccount.kind, managedEditingAccount.platform)}</span>{managedEditingAccount.kind === "apikey" && managedEditingAccount.has_api_key && <span className="text-xs text-success-text">已保存密钥</span>}</div>
              <div className="space-y-5">
                {!managedReauth && <Field id="managed-account-name" label="账户名称"><Input id="managed-account-name" value={managedAccountName} onChange={event => { setManagedAccountName(event.target.value); setManagedError("") }} placeholder="例如：工作订阅" required /></Field>}
                {managedEditingAccount.kind === "apikey" && <>
                  {!managedReauth && (!managedTarget.managed ? <SharedProviderAddress url={managedTarget.base_url} /> : <Field id="managed-account-url" label="Base URL"><Input id="managed-account-url" type="url" inputMode="url" spellCheck={false} value={managedAccountBaseUrl} onChange={event => { setManagedAccountBaseUrl(event.target.value); setManagedError("") }} placeholder="https://api.openai.com/v1" required /></Field>)}
                  {managedReauth && <Field id="managed-account-key" label="替换 API Key" hint="保存后，此账户的后续请求使用新密钥"><Input id="managed-account-key" type="password" autoComplete="off" spellCheck={false} value={managedAccountKey} onChange={event => { setManagedAccountKey(event.target.value); setManagedError("") }} placeholder="输入新的 API Key" required /></Field>}
                </>}
                {managedReauth && managedEditingAccount.kind === "oauth" && <div className="space-y-4">
                  {managedReauth && managedEditingAccount.platform === "gemini" && <>
                    <Field id="managed-oauth-type" label="Gemini 账户类型"><SelectField id="managed-oauth-type" value={managedOauthType} onValueChange={value => { setManagedOauthType(value); setManagedOauthStart(null); setManagedOauthCode("") }} options={geminiAccountTypes} /></Field>
                    <Field id="managed-oauth-project" label="Google Cloud Project ID" hint="留空会尝试自动识别"><Input id="managed-oauth-project" value={managedOauthProjectId} onChange={event => { setManagedOauthProjectId(event.target.value); setManagedOauthStart(null) }} placeholder="my-google-cloud-project" /></Field>
                    {managedOauthType === "ai_studio" && <><Field id="managed-oauth-client-id" label="OAuth Client ID"><Input id="managed-oauth-client-id" value={managedOauthClientId} onChange={event => { setManagedOauthClientId(event.target.value); setManagedOauthStart(null) }} /></Field><Field id="managed-oauth-client-secret" label="OAuth Client Secret" hint={managedEditingAccount.has_client_secret ? "出于安全原因，请在重新授权时再次填写" : undefined}><Input id="managed-oauth-client-secret" type="password" autoComplete="off" value={managedOauthClientSecret} onChange={event => { setManagedOauthClientSecret(event.target.value); setManagedOauthStart(null) }} /></Field></>}
                  </>}
                  {managedReauth && managedEditingAccount.platform === "antigravity" && <Field id="managed-oauth-project" label="Google Cloud Project ID" hint="留空会尝试自动识别"><Input id="managed-oauth-project" value={managedOauthProjectId} onChange={event => { setManagedOauthProjectId(event.target.value); setManagedOauthStart(null) }} placeholder="my-google-cloud-project" /></Field>}
                  <section className="rounded-xl bg-bg-surface p-3.5 ring-1 ring-inset ring-[var(--glass-border-subtle)]">
                    <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                      <div><h3 className="text-sm font-semibold">OAuth 授权</h3><p className="mt-1 text-xs leading-5 text-muted-foreground">重新授权成功前，当前凭证会继续保留。</p></div>
                      <Button type="button" variant="outline" onClick={managedReauth ? startManagedOAuth : () => setManagedReauth(true)} disabled={managedOauthStarting || managedSaving}>{managedOauthStarting && <RotateCw className="animate-spin" aria-hidden="true" />}{managedOauthStart ? "重新生成并打开授权页" : managedReauth ? "打开授权页面" : "重新授权"}<ExternalLink aria-hidden="true" /></Button>
                    </div>
                  </section>
                  {managedOauthStart && <div className="space-y-3">
                    <p className="text-xs leading-5 text-muted-foreground">需要继续登录时，请打开本次授权页。重新生成会切换会话，旧回调将不能在此提交。</p>
                    <OAuthStep number={1} title="登录并复制授权结果" complete={managedOauthCodeReady}><p className="mb-3 text-xs leading-5 text-muted-foreground">授权后复制授权码或地址栏中的完整回调地址。localhost 页面无法连接时，也可以复制地址继续。仅使用本次 JunoHub 授权的结果。</p><a href={managedOauthStart.auth_url} target="_blank" rel="noreferrer" className="inline-flex items-center gap-2 text-sm font-medium text-accent-text hover:underline">打开本次授权页<ExternalLink className="size-4" strokeWidth={1.75} aria-hidden="true" /></a></OAuthStep>
                    <OAuthStep number={2} title="确认新的授权结果" complete={managedOauthCodeReady}><Field id="managed-oauth-result" label="回调地址或授权码" hint="粘贴本次授权结果，提交后验证并保存新凭证"><Input id="managed-oauth-result" value={managedOauthCode} onChange={event => { setManagedOauthCode(event.target.value); setManagedError("") }} placeholder="粘贴授权结果" autoComplete="off" spellCheck={false} /></Field><FormError message={managedOauthResult?.error || ""} /></OAuthStep>
                  </div>}
                </div>}
              </div>
            </fieldset>
            {capacityDirty && <div className="rounded-xl bg-accent-subtle p-4 text-sm leading-6">账户容量还有未保存的修改。请先处理，再保存请求分配。<Button type="button" variant="outline" size="sm" className="mt-2 block" onClick={() => setManagedTab("capacity")}>处理账户容量修改</Button></div>}
            <FormError message={managedError} />
            {!managedReady && !managedLoading && <Button type="button" variant="outline" onClick={() => managedTarget && openEdit(managedTarget)}>重新加载</Button>}
            <div className="flex gap-3 border-t border-[var(--glass-border-subtle)] pt-5"><Button type="button" variant="outline" className="flex-1" disabled={managedSaving || managedOauthStarting} onClick={leaveAccount}>取消</Button><Button type="submit" className="flex-1" disabled={managedSaving || (managedReauth && (managedEditingAccount.kind === "oauth" ? !managedOauthCodeReady : !managedAccountKey.trim()))}>{managedSaving && <RotateCw className="animate-spin" aria-hidden="true" />}{managedReauth ? "保存新凭证" : "保存账户资料"}</Button></div>
          </AppForm>}
        </FormDialog>

        <div hidden={managedTab !== "capacity"}>
          {managedReady && managedTarget ? <AccountDispatchEditor key={`${managedTarget.id}-${capacityRevision}-${managedAccounts.map(a => `${a.id}:${a.priority}:${a.weight}:${a.concurrency}:${a.rpm}`).join()}`} providerId={managedTarget.id} accounts={managedAccounts} onSaved={setManagedAccounts} onDirtyChange={setCapacityDirty} onBusyChange={setCapacityBusy} /> : <p role="status" className="py-8 text-sm text-muted-foreground">{managedLoading ? "正在加载账户配置…" : "账户配置加载失败，请返回授权账户重新加载。"}</p>}
        </div>
        <div hidden={managedTab !== "policy"}>
          <AppForm onSubmit={savePolicy} className="space-y-5" aria-busy={managedSaving || managedLoading}>
            <DispatchPolicyEditor key={`${JSON.stringify(savedPolicy)}-${policyRevision}`} value={policy} onChange={setPolicy} disabled={managedSaving || managedLoading || !managedReady || capacityDirty} accountCount={managedAccounts.length} direct={!managedTarget.managed}><RoutingAccountsEditor accounts={routingAccounts} strategy={policy.strategy} onChange={setRoutingDraft} onInvalidChange={setRoutingInvalid} maxAttempts={policy.max_attempts} /></DispatchPolicyEditor>
            {capacityDirty && <div className="rounded-xl bg-accent-subtle p-4 text-sm leading-6">账户容量还有未保存的修改。请先处理，再保存请求分配。<Button type="button" variant="outline" size="sm" className="mt-2 block" onClick={() => setManagedTab("capacity")}>处理账户容量修改</Button></div>}
            <FormError message={managedError} />
            {!managedReady && !managedLoading && <Button type="button" variant="outline" onClick={() => managedTarget && openEdit(managedTarget)}>重新加载</Button>}
            <div className="sticky bottom-0 z-10 flex flex-wrap items-center justify-between gap-3 border-t border-border-default bg-bg-surface py-4"><p role="status" className="text-xs text-muted-foreground">{routingInvalid ? "空分组需要加入账户或移除后才能保存" : policyDirty ? "修改尚未生效" : "与已保存配置一致"}</p><div className="flex gap-2"><Button type="button" variant="outline" disabled={!policyDirty || managedSaving} onClick={() => { if (savedPolicy) setPolicy({ ...savedPolicy }); setRoutingDraft({}); setPolicyRevision(n => n + 1) }}>撤销修改</Button><Button type="submit" disabled={managedSaving || managedLoading || !managedReady || !policyDirty || capacityDirty || routingInvalid}>{managedSaving && <RotateCw className="animate-spin" aria-hidden="true" />}保存请求分配</Button></div></div>
          </AppForm>
        </div>
        <div hidden={managedTab !== "accounts"}>
          {<div className="mb-5 flex flex-wrap items-center justify-between gap-3 border-b border-border-default pb-4"><div className="min-w-0"><p className="text-sm font-semibold">供应商资料</p><p className="mt-1 break-words text-sm">{managedTarget.name}</p><p className="mt-1 break-all text-xs text-muted-foreground">{managedTarget.managed ? MANAGED_POOL_LABEL : managedTarget.base_url}</p></div><Button type="button" variant="outline" size="sm" disabled={managedSaving || capacityBusy} onClick={() => editProviderConnection(managedTarget)}>编辑供应商</Button></div>}
          <div className="space-y-5" aria-busy={managedLoading || managedSaving}>
            <div className="flex flex-wrap items-center justify-between gap-3"><div><h2 className="text-base font-semibold">{managedTarget.managed ? "已连接的账户" : "已配置的 Key"}</h2><p className="mt-1.5 text-sm text-muted-foreground">每份凭证独立保存，可单独更新或移除。</p></div></div>
            {managedLoading ? (
              <div className="flex min-h-40 items-center justify-center gap-2 text-sm text-muted-foreground"><RotateCw className="size-4 animate-spin" aria-hidden="true" />正在加载账户</div>
            ) : managedAccounts.length > 0 ? (
              <ol className="divide-y divide-border-default">
                {managedAccounts.map(account => (
                  <li key={account.id} className="py-5 first:pt-1 last:pb-1">
                    <div className="flex flex-wrap items-start gap-3">
                      <span className="grid size-10 shrink-0 place-items-center rounded-xl bg-accent-subtle text-accent-text"><KeyRound className="size-4" aria-hidden="true" /></span>
                      <div className="min-w-0 flex-1"><h3 className="text-sm font-semibold [overflow-wrap:anywhere]">{account.name}</h3><p className="mt-1 text-xs text-muted-foreground">{accountKindLabel(account.kind, account.platform)}</p>{account.base_url && <p className="mt-2 text-xs text-muted-foreground [overflow-wrap:anywhere]">{account.base_url}</p>}</div>
                      <AccountHealth account={account} />
                    </div>
                    <div className="mt-3 flex flex-wrap items-center justify-between gap-2 sm:ps-13">
                      <div className="flex items-center gap-1"><Button type="button" size="sm" variant="ghost" disabled={managedSaving} onClick={() => openManagedAccountEdit(account)} aria-label={`编辑 ${account.name}`}><Pencil aria-hidden="true" />编辑资料</Button><Button type="button" size="sm" variant="ghost" disabled={managedSaving} onClick={() => openManagedAccountEdit(account, true)} aria-label={`更新凭证 ${account.name}`}>{account.kind === "oauth" ? "重新授权" : "更换密钥"}</Button><Button type="button" size="sm" variant="ghost" disabled={managedSaving} className="text-muted-foreground hover:bg-danger-subtle hover:text-danger-text" onClick={() => withCleanCapacity(() => setAccountDeleteTarget(account))} aria-label={`删除账户 ${account.name}`}><Trash2 aria-hidden="true" />删除</Button></div>
                      <span className="text-xs text-muted-foreground">最多同时处理 {account.concurrency || 1} 个请求</span>
                    </div>
                  </li>
                ))}
              </ol>
            ) : !managedError && <div className="flex min-h-40 items-center justify-center text-sm text-muted-foreground">这个供应商还没有账户</div>}
            {capacityDirty && <div className="rounded-xl bg-accent-subtle p-4 text-sm leading-6">账户容量还有未保存的修改。请先处理，再保存请求分配。<Button type="button" variant="outline" size="sm" className="mt-2 block" onClick={() => setManagedTab("capacity")}>处理账户容量修改</Button></div>}
            <FormError message={managedError} />
            {!managedReady && !managedLoading && <Button type="button" variant="outline" onClick={() => managedTarget && openEdit(managedTarget)}>重新加载</Button>}
            <div className="flex justify-end border-t border-border-default pt-4"><Button type="button" variant="outline" disabled={managedLoading || capacityDirty} onClick={async () => { if (!managedTarget) return; setManagedLoading(true); try { const data = await consoleAPI.providerAccounts(managedTarget.id); setManagedAccounts(data.list || []); setManagedError("") } catch { setManagedError("无法刷新账户状态，请重试。") } finally { setManagedLoading(false) } }}>刷新状态</Button></div>
          </div>
        </div>
        </div></div>
      </section>}

      <FormDialog open={Boolean(testReport)} onOpenChange={open => { if (!open && testingId === null) setTestReport(null) }} title={testReport ? `${providerPresentation(testReport.provider).title} · 连接测试` : "连接测试"} description="检查各账户的连接情况，查看需要处理的问题。" contentClassName="sm:max-w-xl">
        {testReport && <div className="space-y-5">
          <div className="rounded-xl border border-border-default bg-bg-sunken px-4">{testingId !== null ? <div role="status" className="flex items-center gap-3 py-5"><RotateCw className="size-5 shrink-0 animate-spin text-accent-text motion-reduce:animate-none" aria-hidden="true" /><div><p className="text-sm font-semibold">正在验证连接</p><p className="mt-1 text-xs text-muted-foreground">逐一检查账户，请稍候…</p></div></div> : <ConnectionTestResultView report={testReport.report.checks?.length === 1 ? testReport.report.checks[0] : testReport.report} name={testReport.report.checks?.length === 1 ? testReport.report.checks[0].name : undefined} onUseAddress={!testReport.provider.managed ? url => editProviderConnection(testReport.provider, url) : undefined} addressActionLabel="前往编辑供应商" />}</div>
          {(testReport.report.checks?.length || 0) > 1 && <section aria-label="各 Key 测试结果"><div className="flex items-center justify-between gap-3"><h3 className="text-sm font-semibold">连接明细</h3><span className="text-xs tabular-nums text-muted-foreground">{testReport.report.checks?.length} 个 Key</span></div><div className="mt-3 divide-y divide-border-default rounded-xl border border-border-default px-3">{testReport.report.checks?.map((check, index) => <ConnectionTestResultView key={index} report={check} name={check.name} onUseAddress={!testReport.provider.managed ? url => editProviderConnection(testReport.provider, url) : undefined} addressActionLabel="前往编辑供应商" />)}</div></section>}
          {!testReport.provider.managed && <div className="border-t border-border-default pt-3"><button type="button" disabled={testingId !== null} aria-expanded={testModelOpen} onClick={() => setTestModelOpen(open => !open)} className="flex min-h-11 w-full items-center justify-between gap-3 rounded-lg text-start focus-visible:outline-2 focus-visible:outline-ring"><span><span className="block text-sm font-semibold">验证实际模型调用</span><span className="mt-1 block text-xs text-muted-foreground">连接检查后，可进一步确认模型权限</span></span><ChevronDown aria-hidden="true" className={cn("size-4 shrink-0 text-muted-foreground transition-transform", testModelOpen && "rotate-180")} /></button>{testModelOpen && <div className="mt-4 space-y-3">{testReport.provider.provider_type !== "gemini" && <ModelTestSettings value={testSettings} onChange={value => { setTestSettings(value); setTestReport(current => current ? { ...current, report: { ok: false, outcome: "warning", status_code: 0, message: "测试配置已修改，请重新验证。" } } : current) }} disabled={testingId !== null} prefix="saved-provider-test" />}<UpstreamModelPicker key={testReport.provider.id} providerId={testReport.provider.id} value={testModel} onChange={setTestModel} purpose="test" disabled={testingId !== null} /><p className="text-xs leading-5 text-muted-foreground">{["gpt-image-2", "gpt-image-2.5-sunburst", "gpt-image-2.5-flare"].includes(testModel.trim()) ? "图片测试会生成 1 张低质量图片并产生费用，最多等待约 2 分钟。" : "验证会发送简短生成请求，可能产生少量用量。"}</p><div className="flex flex-wrap items-center justify-between gap-3"><p className="text-xs text-muted-foreground">逐一验证各 Key 的调用权限</p><Button type="button" size="sm" variant="outline" disabled={testingId !== null || !testModel.trim()} onClick={() => testProvider(testReport.provider, testModel.trim())}>{testingId !== null && <RotateCw className="animate-spin motion-reduce:animate-none" aria-hidden="true" />}验证模型调用</Button></div></div>}</div>}
          <div className="flex flex-wrap items-center justify-between gap-3 border-t border-border-default pt-4">{!testReport.provider.managed ? <Button type="button" variant="ghost" size="sm" disabled={testingId !== null} onClick={() => editProviderConnection(testReport.provider)}>编辑供应商</Button> : <p className="text-xs text-muted-foreground">仅测试，不更改配置</p>}<Button type="button" variant="outline" size="sm" disabled={testingId !== null} onClick={() => testProvider(testReport.provider)}><RotateCw aria-hidden="true" />重新检查连接</Button></div>
        </div>}
      </FormDialog>
      <ConfirmDialog open={Boolean(pendingLeave)} onOpenChange={open => { if (!open) setPendingLeave(null) }} title="放弃未保存的修改？" description="已保存的配置继续生效。继续操作将清除当前编辑中的未保存修改。" confirmText="放弃修改" cancelText="继续编辑" onConfirm={() => { pendingLeave?.(); setPendingLeave(null) }} />
      {accountDeleteTarget && managedTarget && <DeleteProviderAccountDialog providerId={managedTarget.id} account={accountDeleteTarget} lastAccount={managedAccounts.length === 1} onClose={() => setAccountDeleteTarget(null)} onDeleted={id => { setManagedAccounts(current => current.filter(account => account.id !== id)); setAccountDeleteTarget(null) }} />}
      {deleteTarget && <DeleteProviderDialog provider={deleteTarget} onClose={() => setDeleteTarget(null)} onDeleted={() => { setDeleteTarget(null); void load() }} />}
    </div>
  )
}

function OAuthStep({ number, title, complete = false, children }: { number: number; title: string; complete?: boolean; children: React.ReactNode }) {
  return (
    <section className="rounded-xl bg-bg-surface p-3.5 ring-1 ring-inset ring-[var(--glass-border-subtle)]">
      <div className="flex items-center gap-2.5">
        <span className={cn("grid size-6 shrink-0 place-items-center rounded-full text-xs font-semibold", complete ? "bg-success-subtle text-success-text" : "bg-accent-subtle text-accent-text")} aria-hidden="true">{complete ? <Check className="size-3.5" strokeWidth={2} /> : number}</span>
        <h3 className="text-sm font-semibold">{title}</h3>
      </div>
      <div className="mt-2.5 ps-8.5">{children}</div>
    </section>
  )
}

function PlatformButton({ platform, selected, onClick }: { platform: Omit<typeof accountPlatforms[number], "description"> & { description: string }; selected: boolean; onClick: () => void }) {
  const Icon = platform.icon
  return (
    <button type="button" aria-pressed={selected} onClick={onClick} className={cn("flex min-h-18 items-center gap-3 rounded-xl border p-3 text-start transition-[border-color,background-color,scale] duration-150 ease-out active:scale-[0.96] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring", selected ? "border-accent-solid/50 bg-accent-subtle" : "border-border-default bg-bg-surface hover:border-border-strong hover:bg-bg-sunken")}>
      <span className={cn("grid size-9 shrink-0 place-items-center rounded-lg", selected ? "bg-bg-surface text-accent-text" : "bg-bg-sunken text-muted-foreground")}><Icon className="size-4.5" strokeWidth={1.75} aria-hidden="true" /></span>
      <span className="min-w-0 flex-1"><span className="block text-sm font-semibold leading-5">{platform.label}</span><span className="mt-0.5 block text-xs leading-4 text-muted-foreground">{platform.description}</span></span>
      {selected && <Check className="size-4 shrink-0 text-accent-text" strokeWidth={2} aria-hidden="true" />}
    </button>
  )
}

function AuthMethodButton({ selected, title, description, onClick }: { selected: boolean; title: string; description: string; onClick: () => void }) {
  return <button type="button" aria-pressed={selected} onClick={onClick} className={cn("relative min-h-18 rounded-xl border p-3 pe-9 text-start transition-[border-color,background-color,scale] duration-150 ease-out active:scale-[0.96] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring", selected ? "border-accent-solid/50 bg-accent-subtle" : "border-border-default bg-bg-surface hover:border-border-strong hover:bg-bg-sunken")}><span className="text-sm font-semibold">{title}</span><span className="mt-1 block text-xs leading-4 text-muted-foreground">{description}</span>{selected && <Check className="absolute end-3 top-3 size-4 text-accent-text" strokeWidth={2} aria-hidden="true" />}</button>
}

function KimiIcon(props: React.SVGProps<SVGSVGElement>) {
  return <svg viewBox="0 0 1024 1024" xmlns="http://www.w3.org/2000/svg" fill="currentColor" {...props}><path d="M585.216 833.331V457.523a119.45 119.45 0 0 1 119.398 119.347v256.41zm-423.834-8.857V218.982h121.14v605.492zm121.805-247.604V457.523h302.08v35.124a84.326 84.326 0 0 1-84.377 84.223zm149.248-119.347 103.629-242.33H669.49l-79.923 186.88a91.29 91.29 0 0 1-83.968 55.45z" /><path d="M862.566 260.915a70.246 70.246 0 0 1-70.246 70.247c-10.24 0-64.973 7.014-64.973 7.014s10.548-12.288 19.303-23.859a70.246 70.246 0 1 1 115.865-53.35z" /></svg>
}

function GlmIcon(props: React.SVGProps<SVGSVGElement>) {
  return <svg viewBox="0 0 1024 1024" xmlns="http://www.w3.org/2000/svg" fill="currentColor" {...props}><path d="M611.533 763.597a103.834 103.834 0 0 1 119.808-43.418c52.992 16.896 86.17 74.547 68.198 129.382-17.971 53.965-73.728 83.661-130.406 67.226-25.344-7.322-44.237-26.983-57.14-43.418-16.128-20.121-28.108-31.59-57.6-39.321-47.923-12.8-107.366-2.304-131.328 38.4a102.4 102.4 0 0 1-66.355 46.592 104.346 104.346 0 0 1-123.955-77.261 103.22 103.22 0 0 1 78.797-124.775c3.686-.921 8.294-1.382 12.902-1.843 7.834-.921 16.589-1.843 24.883-5.017 35.482-12.34 64.052-55.808 62.669-88.73a76.288 76.288 0 0 0-12.902-37.888c-28.109-48.486-33.639-116.173 10.137-162.304 23.501-24.73 55.296-38.912 89.395-42.086 4.148-.512 9.216 0 14.285 0h13.363c40.09-1.844 81.101-54.426 78.797-98.304a60.416 60.416 0 0 0-3.686-20.583c-12.903-32.46-30.874-41.574-59.443-44.8-48.845-5.017-103.68-.46-128.103 43.879l-1.382 1.843a104.704 104.704 0 0 1-120.269 43.418c-52.992-16.436-85.709-74.496-67.277-130.304a103.987 103.987 0 0 1 129.383-67.226c25.344 7.322 44.237 26.983 57.14 44.34 16.128 20.121 28.108 31.539 57.6 39.321 47.923 12.8 107.366 2.304 131.328-38.4a102.4 102.4 0 0 1 66.355-46.592 104.346 104.346 0 0 1 123.955 77.21 103.22 103.22 0 0 1-78.797 124.825c-3.686.922-8.294 1.382-12.902 1.843-7.834.87-16.589 1.792-24.883 5.018-35.482 12.288-64.052 55.808-62.669 88.678.922 14.183 5.53 26.983 12.902 37.94 11.52 20.12 18.432 42.086 21.197 61.286.46 3.174.921 12.288.46 19.66-1.843 35.636-14.284 57.6-31.334 78.183-18.432 22.374-60.825 47.514-104.601 45.722a59.238 59.238 0 0 0-13.363-.512c-40.09 1.843-81.101 54.425-78.797 98.304 0 7.321 1.382 14.182 3.686 20.582 12.903 32.461 30.874 41.626 59.443 44.8 48.845 5.069 103.68.512 128.103-43.878l1.382-1.844zm257.126-354.765a103.936 103.936 0 0 0-104.14 103.322c0 57.139 46.54 103.321 104.14 103.321A103.936 103.936 0 0 0 972.8 512.154a103.629 103.629 0 0 0-104.141-103.322zM259.942 512.614a103.322 103.322 0 0 0-104.14-103.321C98.202 409.293 51.2 455.475 51.2 512.614c0 57.6 46.54 103.322 104.14 103.322 57.6 0 104.602-46.182 104.602-103.322z" /></svg>
}

function DeepSeekIcon(props: React.SVGProps<SVGSVGElement>) {
  return <svg viewBox="0 0 1024 1024" xmlns="http://www.w3.org/2000/svg" fill="currentColor" {...props}><path d="M929.678 254.123c-9.045-4.324-12.914 3.982-18.204 8.192-1.764 1.365-3.3 3.186-4.836 4.779-13.255 13.937-28.672 23.096-48.81 21.959-29.469-1.593-54.614 7.509-76.914 29.81-4.721-27.421-20.48-43.805-44.373-54.387-12.516-5.46-25.145-10.922-33.906-22.755-6.144-8.476-7.794-17.92-10.866-27.193-1.934-5.518-3.868-11.32-10.41-12.288-7.112-1.08-9.9 4.779-12.687 9.671-11.093 20.082-15.36 42.269-15.019 64.626.968 50.346 22.528 90.51 65.366 119.011 4.835 3.3 6.144 6.6 4.55 11.378-2.844 9.785-6.37 19.342-9.443 29.24-1.877 6.258-4.835 7.624-11.662 4.893a196.608 196.608 0 0 1-61.724-41.415c-30.436-29.014-58.027-61.099-92.331-86.244a403.285 403.285 0 0 0-24.462-16.498c-35.044-33.564 4.551-61.212 13.71-64.455 9.672-3.413 3.357-15.19-27.647-15.019-31.005.114-59.392 10.354-95.574 24.007a109.682 109.682 0 0 1-16.498 4.78 345.657 345.657 0 0 0-102.513-3.585c-67.015 7.396-120.548 38.685-159.915 92.047-47.274 64.17-58.424 137.102-44.828 213.106 14.393 80.156 55.808 146.488 119.467 198.428 66.105 53.76 142.222 80.1 228.921 75.094 52.736-3.015 111.388-10.012 177.607-65.308 16.725 8.192 34.19 11.491 63.318 13.937 22.357 2.048 43.918-1.08 60.586-4.494 26.169-5.462 24.292-29.355 14.905-33.678-76.687-35.271-59.79-20.935-75.094-32.54 38.912-45.512 97.565-92.729 120.548-245.76 1.763-12.175.284-19.798 0-29.583-.114-6.03 1.251-8.362 8.192-9.102 19.228-1.934 37.888-7.566 54.954-16.611 49.607-26.738 69.69-70.713 74.411-123.45.683-7.964-.17-16.383-8.818-20.593zM497.038 728.405c-74.24-57.57-110.25-76.572-125.155-75.719-13.88.797-11.378 16.498-8.306 26.738 3.243 10.126 7.396 17.066 13.255 25.941 3.983 5.86 6.827 14.62-4.039 21.163-23.893 14.62-65.536-4.95-67.47-5.86-48.356-28.16-88.86-65.308-117.362-116.053a350.948 350.948 0 0 1-46.194-157.298c-.682-13.539 3.357-18.318 17.01-20.707 17.92-3.3 36.409-3.982 54.329-1.365 75.776 10.922 140.345 44.373 194.39 97.28 30.89 30.151 54.272 66.218 78.279 101.49 25.6 37.375 53.191 73.044 88.177 102.228 12.402 10.24 22.244 18.034 31.744 23.78-28.5 3.128-76.06 3.81-108.658-21.618zm35.67-225.905a10.809 10.809 0 0 1 14.677-10.127 9.728 9.728 0 0 1 4.096 2.56 10.809 10.809 0 0 1-7.965 18.319 10.695 10.695 0 0 1-10.808-10.752zm110.535 55.978a65.024 65.024 0 0 1-21.049 5.518 44.658 44.658 0 0 1-28.33-8.817c-9.671-8.079-16.668-12.516-19.627-26.624a59.278 59.278 0 0 1 .57-20.594c2.502-11.491-.285-18.887-8.534-25.6-6.6-5.404-15.133-6.94-24.462-6.94a20.082 20.082 0 0 1-8.989-2.731c-3.868-1.877-7.11-6.656-3.982-12.516a40.05 40.05 0 0 1 6.826-7.395c12.573-7.054 27.193-4.779 40.676.569 12.515 5.006 21.959 14.336 35.612 27.42 13.938 15.815 16.384 20.252 24.348 32.085 6.258 9.33 11.947 18.887 15.872 29.81 2.276 6.826-.74 12.344-8.931 15.758v.057z" /></svg>
}

function CustomIcon(props: React.SVGProps<SVGSVGElement>) {
  return <svg viewBox="0 0 1024 1024" xmlns="http://www.w3.org/2000/svg" fill="currentColor" {...props}><path d="M512 928c-125.44 0-224-182.72-224-416S386.56 96 512 96s224 182.72 224 416-98.56 416-224 416zm0-768c-75.84 0-160 144.64-160 352s84.16 352 160 352 160-144.64 160-352-84.16-352-160-352z" /><path d="M699.52 797.76A623.04 623.04 0 0 1 400 705.92C198.08 589.44 88.96 412.8 151.68 304S422.08 201.28 624 318.08 935.04 611.2 872.32 720c-30.08 51.84-93.12 77.76-172.8 77.76zM324.16 290.56c-57.6 0-100.16 16.32-117.12 45.44-37.76 65.6 45.44 210.88 224 314.56S777.92 753.6 815.04 688 769.6 477.12 591.04 373.44a560.32 560.32 0 0 0-266.88-82.88z" /><path d="M324.48 797.76c-79.68 0-142.72-25.92-172.8-77.76-64-108.8 46.4-285.44 248.32-401.92S809.6 195.2 872.32 304 825.92 589.44 624 705.92a623.04 623.04 0 0 1-299.52 91.84zm375.36-507.2A560.32 560.32 0 0 0 432 373.44C252.48 477.12 169.28 622.4 208 688s205.44 66.24 384-37.44S854.72 401.6 816 336c-16-29.12-58.56-45.44-116.16-45.44z" /><path d="M512 592a80 80 0 1 1 80-80 80 80 0 0 1-80 80zm0-128a48 48 0 1 0 48 48 48 48 0 0 0-48-48z" /></svg>
}

function ClaudeIcon(props: React.SVGProps<SVGSVGElement>) {
  return <svg viewBox="0 0 1024 1024" xmlns="http://www.w3.org/2000/svg" fill="currentColor" {...props}><path d="M202.112 678.656l200.64-112.64 3.392-9.792-3.392-5.44h-9.792l-33.6-2.048-114.624-3.072-99.456-4.224-96.384-5.12-24.192-5.12-22.72-29.952 2.304-14.976 20.48-13.696 29.12 2.56 64.576 4.416 96.832 6.72 70.208 4.096 104.064 10.88h16.576l2.304-6.72-5.696-4.16-4.352-4.096-100.224-67.968-108.48-71.744-56.768-41.344-30.72-20.928-15.488-19.584-6.72-42.88 27.84-30.72 37.504 2.56 9.536 2.56 37.952 29.184 81.088 62.784 105.856 77.952 15.488 12.928 6.208-4.352.768-3.136L395.264 360l-57.6-104.064-61.44-105.92-27.392-43.904-7.168-26.304c-2.56-10.88-4.48-19.904-4.48-30.976l31.808-43.136L286.592 0l42.304 5.696 17.856 15.488 26.304 60.16 42.624 94.72 66.112 128.896 19.392 38.208 10.24 35.392 3.904 10.88h6.72v-6.208l5.44-72.576 10.048-89.088 9.856-114.688 3.328-32.256 16-38.72 31.808-20.928 24.768 11.904 20.416 29.184-2.88 18.816-12.16 78.72-23.68 123.52-15.552 82.56h9.088l10.304-10.24 41.856-55.552 70.208-87.808 30.976-34.88 36.16-38.464 23.232-18.368h43.904l32.32 48.064-14.464 49.6-45.184 57.28-37.44 48.576-53.76 72.32-33.536 57.856 3.072 4.608 8-.768 121.408-25.792 65.6-11.904 78.208-13.44 35.392 16.512 3.84 16.832-13.952 34.304-83.648 20.672-98.112 19.648-146.176 34.56-1.792 1.28 2.048 2.56 65.92 6.272 28.096 1.536h68.928l128.384 9.6 33.536 22.144 20.16 27.136-3.392 20.672-51.648 26.304-69.696-16.512-162.688-38.72-55.744-13.952h-7.744v4.672l46.464 45.44 85.184 76.928 106.688 99.2 5.376 24.512-13.632 19.328-14.464-2.048-93.76-70.464-36.16-31.808-81.856-68.928h-5.44v7.232l18.88 27.648 99.648 149.76 5.184 45.952-7.232 14.976-25.856 9.024-28.352-5.12L673.408 856l-60.16-92.16-48.576-82.624-5.952 3.392-28.672 308.544-13.44 15.744-30.976 11.904-25.792-19.648-13.696-31.744 13.696-62.72 16.512-81.92 13.44-65.024 12.16-80.832 7.232-26.88-.512-1.792-5.952.768-60.928 83.648-92.736 125.248-73.344 78.528-17.536 6.976-30.464-15.808 2.816-28.16 17.024-24.96 101.504-129.152 61.184-80 39.552-46.272-.256-6.72h-2.368L177.6 789.44l-48 6.144-20.736-19.328 2.56-31.744 9.856-10.368 81.088-55.744-.256.256z" /></svg>
}

function GeminiIcon(props: React.SVGProps<SVGSVGElement>) {
  return <svg viewBox="0 0 1024 1024" xmlns="http://www.w3.org/2000/svg" fill="currentColor" {...props}><path d="M938.496 512.768c-228.864 13.824-411.989 196.949-425.813 425.813h-1.707c-13.653-228.864-196.821-411.989-425.643-425.813v-1.707c228.822-13.653 411.99-196.821 425.814-425.642h1.706c13.824 228.821 196.992 411.989 425.814 425.813v1.536h-.171z" /></svg>
}

function GrokIcon(props: React.SVGProps<SVGSVGElement>) {
  return <svg viewBox="0 0 1024 1024" xmlns="http://www.w3.org/2000/svg" fill="currentColor" {...props}><path d="M395.52 652.373 735.915 400.768c16.683-12.373 40.533-7.552 48.512 11.605 41.813 101.078 23.125 222.507-60.16 305.878-83.243 83.37-199.126 101.632-305.024 59.989l-115.67 53.632c165.931 113.536 367.403 85.461 493.312-40.661 99.883-100.011 130.816-236.331 101.888-359.254l.256.299c-41.941-180.566 10.326-252.758 117.334-400.342L1024 21.333 883.157 162.347v-.427L395.392 652.459m-70.144 61.056c-119.125-113.92-98.56-290.176 3.03-391.851 75.135-75.221 198.271-105.941 305.738-60.8L749.44 207.531a333.141 333.141 0 0 0-78.037-42.667 382.933 382.933 0 0 0-416.086 83.883c-108.074 108.203-142.08 274.603-83.712 416.598 43.606 106.112-27.861 181.162-99.84 256.938C46.208 949.163 20.608 976 0 1004.416l325.12-290.773" /></svg>
}

function AntigravityIcon(props: React.SVGProps<SVGSVGElement>) {
  return <svg viewBox="250 0 1024 1024" xmlns="http://www.w3.org/2000/svg" fill="currentColor" {...props}><path d="M522.441 189.481c-7.92 69.712 52.144 133.756 134.145 143.074 82.02 9.318 154.916-39.661 162.817-109.354 7.921-69.693-52.143-133.756-134.144-143.074-82.02-9.299-154.916 39.66-162.818 109.354z" /><path d="M838.622 417.371c20.539 88.465 110.48 143.152 200.887 122.166 90.406-21.005 147.054-109.742 126.495-198.208-20.539-88.446-110.48-143.152-200.886-122.146-90.407 21.005-147.055 109.742-126.496 198.208z" /><path d="M300.647 476.717c25.974 91.3 142.88 138.027 261.164 104.403 118.264-33.643 193.102-134.921 167.147-226.201-25.975-91.3-142.881-138.027-261.165-104.404-118.264 33.624-193.101 134.882-167.146 226.202z" /><path d="M394.509 552.408c65.073 66.47 177.164 62.277 250.351-9.376 73.187-71.634 79.749-183.629 14.676-250.099-65.111-66.47-177.183-62.277-250.37 9.376-73.188 71.634-79.749 183.609-14.657 250.099zM735.014 803.109c20.19 87.009 104.52 141.773 188.307 122.302 83.787-19.452 135.329-105.762 115.12-192.772-20.19-87.028-104.52-141.793-188.307-122.322-83.787 19.452-135.329 105.763-115.12 192.772zM592.134 108.082c-42.32 94.269 4 206.885 103.433 251.515 99.434 44.65 214.36 4.407 256.68-89.882 42.32-94.251-3.98-206.866-103.414-251.497-99.434-44.65-214.36-4.406-256.68 89.883z" /><path d="M478.781 1017.992c-102.443-40.224 72.644-371.12 116.168-482.027 43.544-110.888 161.905-168.195 264.329-127.971 102.462 40.204 223.891 261.727 180.347 372.634-43.524 110.907-458.401 277.568-560.844 237.364zM1099.553 606.144c-27.43 32.012-99.007 15.686-159.886-36.458-60.86-52.163-87.96-120.38-60.53-152.393 27.412-32.012 99.007-15.686 159.867 36.458s87.98 120.361 60.55 152.393z" /><path d="M894.804 443.19c112.79 76.294 242.702 81.341 290.148 11.202 47.426-70.12-5.572-188.832-118.381-265.144-112.79-76.313-242.702-81.341-290.148-11.201-47.426 70.12 5.572 188.831 118.381 265.143zM524.441 319.082c-27.955 67.422-18.831 135.251 20.383 151.5 39.215 16.248 93.649-25.237 121.604-92.64 27.955-67.421 18.831-135.25-20.384-151.499-39.214-16.249-93.648 25.237-121.603 92.64z" /></svg>
}

function OpenAIIcon(props: React.SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 1024 1024" xmlns="http://www.w3.org/2000/svg" {...props}>
      <path fill="currentColor" d="M950.676 419.057a255.347 255.347 0 0 0-22.015-209.482A257.95 257.95 0 0 0 650.915 85.848a258.76 258.76 0 0 0-438.377 92.539A255.347 255.347 0 0 0 41.966 302.114a257.95 257.95 0 0 0 31.7 302.79 255.133 255.133 0 0 0 21.758 209.525 258.163 258.163 0 0 0 277.96 123.727 255.347 255.347 0 0 0 192.373 85.84 258.376 258.376 0 0 0 246.26-179.446 255.56 255.56 0 0 0 170.53-123.727 258.376 258.376 0 0 0-31.871-301.766zM565.757 957.013a190.966 190.966 0 0 1-122.703-44.37l6.016-3.456 203.893-117.67a33.918 33.918 0 0 0 16.725-29.054v-287.43l86.182 49.832a3.03 3.03 0 0 1 1.621 2.219v238.195a192.16 192.16 0 0 1-191.734 191.734zM153.62 780.98a190.71 190.71 0 0 1-22.826-128.59l6.058 3.626 204.065 117.71a32.894 32.894 0 0 0 33.278 0L623.482 629.99v99.494a3.413 3.413 0 0 1-1.408 2.645L415.578 851.206a191.99 191.99 0 0 1-261.96-70.226zM99.86 336.928a191.35 191.35 0 0 1 100.944-84.177v242.206a32.681 32.681 0 0 0 16.554 28.841l248.094 143.14-86.182 49.832a3.242 3.242 0 0 1-3.03 0L170.173 597.907a192.16 192.16 0 0 1-70.31-262.003zM807.963 501.4L559.102 356.895l85.968-49.661a3.242 3.242 0 0 1 3.03 0L854.169 426.31a191.734 191.734 0 0 1-28.841 345.796v-242.25a33.705 33.705 0 0 0-17.365-28.456zm85.756-128.975-6.016-3.627-203.68-118.692a33.108 33.108 0 0 0-33.491 0L401.456 393.842v-99.536a2.816 2.816 0 0 1 1.195-2.602l206.07-118.906a191.99 191.99 0 0 1 284.998 198.816zM354.44 548.842l-86.182-49.661a3.413 3.413 0 0 1-1.621-2.432V259.236a191.99 191.99 0 0 1 314.65-147.32l-6.058 3.413-203.851 117.668a33.918 33.918 0 0 0-16.767 29.055zm46.803-100.901 111.013-63.997 111.226 63.997v127.95l-110.8 63.997-111.226-63.996z" />
    </svg>
  )
}


function providerLabel(type: string) {
  return providerTemplates.find(item => item.id === type)?.label || type
}

function accountKindLabel(kind: string, platform: string) {
  if (kind === "oauth") return platform === "openai" ? "订阅 OAuth" : "OAuth 账户"
  if (kind === "apikey") return "API Key 账户"
  return kind
}

function providerPresentation(item: ConsoleProvider) {
  const platform = item.platform ? accountPlatforms.find(candidate => candidate.id === item.platform) : undefined
  if (item.managed && platform) {
    return {
      icon: platform.icon,
      title: item.name,
      meta: "内置账户池 · 凭证已托管",
    }
  }
  return {
    icon: item.provider_type === "openai"
      ? OpenAIIcon
      : item.provider_type === "openai-compatible"
        ? CustomIcon
        : item.provider_type === "sub2api"
          ? Sparkles
          : Server,
    title: item.name,
    meta: `${providerLabel(item.provider_type)} · ${item.managed ? MANAGED_POOL_LABEL : item.base_url}`,
  }
}

function SharedProviderAddress({ url }: { url: string }) {
  return <div className="space-y-2"><p className="text-sm font-medium">Base URL</p><p className="break-all rounded-lg border border-border-default bg-bg-sunken px-3 py-2.5 font-mono text-sm">{url}</p><p className="text-xs leading-5 text-muted-foreground">使用供应商统一地址，账户内不可修改。</p></div>
}
