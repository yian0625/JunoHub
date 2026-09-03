"use client"

import { useEffect, useState } from "react"
import { Bot, BrainCircuit, Check, Cloud, ExternalLink, ListPlus, Plus, RotateCw, Server, Settings2, Sparkles, Zap } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ConfirmDialog } from "@/components/confirm-dialog"
import { ConsoleProvider, consoleAPI } from "@/lib/api"
import { cn } from "@/lib/utils"
import { toast } from "sonner"
import { AddCard, EmptyGrid, ErrorState, Field, FormDialog, FormError, LoadingState, PageHead, ResourceCard, ResourceGrid } from "../_components/console-ui"
import { useConsoleGuard } from "../_components/use-console-guard"

const providerTemplates = [
  { id: "sub2api", label: "Sub2API", description: "账户池、调度与故障切换", baseUrl: "http://junohub-server:9003/v1", icon: Sparkles },
  { id: "openai", label: "OpenAI", description: "OpenAI 官方接口", baseUrl: "https://api.openai.com/v1", icon: Cloud },
  { id: "openai-compatible", label: "兼容服务", description: "其他 OpenAI 兼容端点", baseUrl: "", icon: Server },
] as const

const accountPlatforms = [
  { id: "openai", label: "OpenAI", description: "OpenAI API Key", baseUrl: "https://api.openai.com/v1", icon: Bot },
  { id: "anthropic", label: "Claude", description: "Anthropic API Key", baseUrl: "https://api.anthropic.com", icon: BrainCircuit },
  { id: "gemini", label: "Gemini", description: "Google AI Studio API Key", baseUrl: "https://generativelanguage.googleapis.com", icon: Sparkles },
  { id: "grok", label: "Grok", description: "xAI API Key", baseUrl: "https://api.x.ai/v1", icon: Zap },
  { id: "antigravity", label: "Antigravity", description: "Antigravity API Key", baseUrl: "", icon: Cloud },
  { id: "kimi", label: "Kimi", description: "Moonshot API Key", baseUrl: "https://api.moonshot.cn/v1", icon: Cloud },
  { id: "zhipu", label: "智谱 GLM", description: "智谱开放平台 API Key", baseUrl: "https://open.bigmodel.cn/api/paas/v4", icon: Cloud },
  { id: "deepseek", label: "DeepSeek", description: "DeepSeek API Key", baseUrl: "https://api.deepseek.com", icon: Cloud },
] as const

const emptyForm = { name: "", provider_type: "openai-compatible", base_url: "", api_key: "" }

export default function ProvidersPage() {
  const isUser = useConsoleGuard()
  const [providers, setProviders] = useState<ConsoleProvider[]>([])
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState("")
  const [formError, setFormError] = useState("")
  const [submitting, setSubmitting] = useState(false)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editTarget, setEditTarget] = useState<ConsoleProvider | null>(null)
  const [testingId, setTestingId] = useState<number | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<{ id: number; name: string } | null>(null)
  const [form, setForm] = useState(emptyForm)
  const [syncTarget, setSyncTarget] = useState<ConsoleProvider | null>(null)
  const [syncModels, setSyncModels] = useState<string[]>([])
  const [selectedModels, setSelectedModels] = useState<string[]>([])
  const [syncLoading, setSyncLoading] = useState(false)
  const [syncSubmitting, setSyncSubmitting] = useState(false)
  const [syncError, setSyncError] = useState("")
  const [accountOpen, setAccountOpen] = useState(false)
  const [accountPlatform, setAccountPlatform] = useState<(typeof accountPlatforms)[number]["id"]>("openai")
  const [accountName, setAccountName] = useState("OpenAI")
  const [accountKey, setAccountKey] = useState("")
  const [accountBaseUrl, setAccountBaseUrl] = useState("https://api.openai.com/v1")
  const [accountSubmitting, setAccountSubmitting] = useState(false)
  const [accountError, setAccountError] = useState("")
  const [accountMethod, setAccountMethod] = useState<"oauth" | "api_key">("oauth")
  const [oauthStarting, setOauthStarting] = useState(false)
  const [oauthStart, setOauthStart] = useState<{ auth_url: string; session_id: string; state: string } | null>(null)
  const [oauthCode, setOauthCode] = useState("")

  const load = async () => {
    setLoading(true); setLoadError("")
    try { const data = await consoleAPI.providerList(); setProviders(data.list || []) }
    catch (error) { const message = error instanceof Error ? error.message : "无法加载供应商，请检查网络后重试。"; setLoadError(message); toast.error(message) }
    finally { setLoading(false) }
  }

  useEffect(() => { if (isUser) load() }, [isUser])

  const openAccount = () => {
    const platform = accountPlatforms[0]
    setAccountPlatform(platform.id); setAccountName(platform.label); setAccountKey(""); setAccountBaseUrl(platform.baseUrl); setAccountMethod("oauth"); setOauthStart(null); setOauthCode(""); setAccountError(""); setAccountOpen(true)
  }

  const selectAccountPlatform = (platform: typeof accountPlatforms[number]) => {
    setAccountPlatform(platform.id); setAccountName(platform.label); setAccountBaseUrl(platform.baseUrl); setAccountMethod(platform.id === "openai" ? "oauth" : "api_key"); setOauthStart(null); setOauthCode(""); setAccountError("")
  }

  const startOAuth = async () => {
    const popup = window.open("about:blank", "openai-oauth", "width=720,height=780")
    setOauthStarting(true); setAccountError("")
    try {
      const result = await consoleAPI.startOpenAIOAuth()
      setOauthStart(result)
      if (popup) popup.location.href = result.auth_url
      else window.open(result.auth_url, "_blank", "noopener,noreferrer")
    } catch (error) {
      popup?.close()
      setAccountError(error instanceof Error ? error.message : "无法开始 OpenAI 授权。")
    } finally { setOauthStarting(false) }
  }

  const createAccount = async (event: React.FormEvent) => {
    event.preventDefault()
    setAccountSubmitting(true); setAccountError("")
    try {
      if (accountPlatform === "openai" && accountMethod === "oauth") {
        if (!oauthStart) { setAccountError("请先打开授权页面。"); return }
        const parsed = parseOAuthResult(oauthCode, oauthStart.state)
        if (!parsed.code) { setAccountError("请输入授权码或完整回调地址。"); return }
        const result = await consoleAPI.completeOpenAIOAuth({ session_id: oauthStart.session_id, state: parsed.state, code: parsed.code, name: accountName.trim() })
        setAccountOpen(false); await load(); toast.success(`OpenAI Codex 已接入，导入 ${result.imported_count} 个模型`)
      } else {
        if (!accountKey.trim()) { setAccountError("请输入 API Key。"); return }
        if (!accountBaseUrl.trim()) { setAccountError("请输入 Base URL。"); return }
        const result = await consoleAPI.createSub2apiApiKeyAccount({ platform: accountPlatform, name: accountName.trim(), api_key: accountKey.trim(), base_url: accountBaseUrl.trim() })
        setAccountOpen(false); await load(); toast.success(`账户已接入，导入 ${result.imported_count} 个模型`)
      }
    } catch (error) { setAccountError(error instanceof Error ? error.message : "无法接入账户，请检查凭证。") }
    finally { setAccountSubmitting(false) }
  }

  const openDialog = () => { setEditTarget(null); setForm(emptyForm); setFormError(""); setDialogOpen(true) }
  const openEdit = (item: ConsoleProvider) => {
    setEditTarget(item)
    setForm({ name: item.name, provider_type: item.provider_type, base_url: item.base_url, api_key: "" })
    setFormError(""); setDialogOpen(true)
  }
  const selectTemplate = (template: typeof providerTemplates[number]) => {
    setForm(current => ({ ...current, name: current.name === "" || providerTemplates.some(item => item.label === current.name) ? template.label : current.name, provider_type: template.id, base_url: template.baseUrl }))
  }

  const saveProvider = async (event: React.FormEvent) => {
    event.preventDefault(); setFormError(""); setSubmitting(true)
    try {
      if (editTarget) {
        const update = { id: editTarget.id, name: form.name.trim(), provider_type: form.provider_type, base_url: form.base_url.trim(), ...(form.api_key ? { api_key: form.api_key } : {}) }
        await consoleAPI.providerUpdate(update)
        setDialogOpen(false); await load(); toast.success("供应商配置已更新")
      } else {
        const created = await consoleAPI.providerCreate({ ...form, name: form.name.trim(), base_url: form.base_url.trim() })
        setDialogOpen(false); await load(); toast.success("供应商已添加")
        void openSync(created)
      }
    } catch (error) { setFormError(error instanceof Error ? error.message : "无法保存供应商，请检查填写内容后重试。") }
    finally { setSubmitting(false) }
  }

  const testProvider = async (item: ConsoleProvider) => {
    setTestingId(item.id)
    try {
      const result = await consoleAPI.providerTest(item.id)
      if (result.ok) toast.success(`${item.name} 连接正常，HTTP ${result.status_code}`)
      else toast.error(`${item.name} 连接失败，HTTP ${result.status_code}`)
    } catch (error) { toast.error(error instanceof Error ? error.message : `无法测试 ${item.name}。`) }
    finally { setTestingId(null) }
  }

  const openSync = async (item: ConsoleProvider) => {
    setSyncTarget(item); setSyncModels([]); setSelectedModels([]); setSyncError(""); setSyncLoading(true)
    try {
      const data = await consoleAPI.providerModels(item.id)
      setSyncModels(data.models || []); setSelectedModels(data.models || [])
    } catch (error) { setSyncError(error instanceof Error ? error.message : "无法读取上游模型，请检查地址和 API Key。") }
    finally { setSyncLoading(false) }
  }

  const importModels = async (event: React.FormEvent) => {
    event.preventDefault()
    if (!syncTarget || !selectedModels.length) { setSyncError("请至少选择一个模型。"); return }
    setSyncSubmitting(true); setSyncError("")
    try {
      const result = await consoleAPI.modelImport({ provider_id: syncTarget.id, models: selectedModels })
      setSyncTarget(null)
      const skipped = result.skipped_count ? `，跳过 ${result.skipped_count} 个已有别名` : ""
      toast.success(`已导入 ${result.imported_count} 个模型${skipped}`)
    } catch (error) { setSyncError(error instanceof Error ? error.message : "无法导入模型路由。") }
    finally { setSyncSubmitting(false) }
  }

  const toggleModel = (model: string) => setSelectedModels(current => current.includes(model) ? current.filter(item => item !== model) : [...current, model])

  const remove = async () => {
    if (!deleteTarget) return
    try { await consoleAPI.providerDelete(deleteTarget.id); setDeleteTarget(null); await load(); toast.success("已删除") }
    catch (error) { toast.error(error instanceof Error ? error.message : "无法删除，请稍后重试。") }
  }

  if (loading && !providers.length) return <LoadingState />
  if (loadError && !providers.length) return <ErrorState message={loadError} onRetry={load} />

  return (
    <div className="view-enter space-y-8">
      <PageHead title="供应商" description="在 JunoHub 中接入上游账户，系统会自动创建隔离分组、凭证和模型路由。" action={<div className="flex flex-wrap gap-2"><Button variant="outline" size="lg" onClick={openAccount}><Sparkles aria-hidden="true" />添加内置账户</Button><Button size="lg" onClick={openDialog}><Plus aria-hidden="true" />添加兼容 API</Button></div>} />

      {providers.length === 0 ? (
        <EmptyGrid title="还没有供应商" description="添加上游账户后，JunoHub 会自动创建供应商和模型路由。" actionLabel="添加内置账户" onAction={openAccount} />
      ) : (
        <ResourceGrid>
          {providers.map(item => (
            <ResourceCard
              key={item.id}
              badge={item.provider_type === "sub2api" ? <Sparkles className="size-[18px]" strokeWidth={1.75} aria-hidden="true" /> : <Server className="size-[18px]" strokeWidth={1.75} aria-hidden="true" />}
              title={item.name}
              meta={`${providerLabel(item.provider_type)} · ${item.base_url}`}
              status={{ active: item.enabled === 1, label: item.enabled === 1 ? "已启用" : "已停用" }}
              actions={<div className="flex items-center gap-1"><Button size="icon-sm" variant="ghost" onClick={() => openEdit(item)} aria-label={`配置 ${item.name}`} title="编辑配置"><Settings2 aria-hidden="true" /></Button><Button size="icon-sm" variant="ghost" onClick={() => openSync(item)} aria-label={`同步 ${item.name} 的模型`} title="同步模型"><ListPlus aria-hidden="true" /></Button><Button size="sm" variant="outline" onClick={() => testProvider(item)} disabled={testingId === item.id}>{testingId === item.id && <RotateCw className="animate-spin" aria-hidden="true" />}测试</Button></div>}
              onDelete={() => setDeleteTarget({ id: item.id, name: item.name })}
              deleteLabel={`删除 ${item.name}`}
            />
          ))}
          <AddCard label="添加供应商" onClick={openDialog} />
        </ResourceGrid>
      )}

      <FormDialog open={accountOpen} onOpenChange={setAccountOpen} title="添加内置账户" description="选择平台与认证方式，JunoHub 会在内部完成账户池、分组和路由配置。">
        <form onSubmit={createAccount} className="space-y-5">
          <div><p className="text-sm font-medium">平台</p><div className="mt-2 grid grid-cols-2 gap-2">{accountPlatforms.map(platform => { const Icon = platform.icon; const selected = accountPlatform === platform.id; return <button key={platform.id} type="button" onClick={() => selectAccountPlatform(platform)} aria-pressed={selected} className={cn("relative min-h-20 rounded-xl border p-3 text-start transition-colors", selected ? "border-accent-solid/40 bg-accent-subtle" : "border-border-default bg-bg-surface hover:border-border-strong hover:bg-bg-sunken")}><Icon className={cn("size-4", selected ? "text-accent-text" : "text-muted-foreground")} aria-hidden="true" /><span className="mt-2 block text-xs font-semibold">{platform.label}</span><span className="mt-1 block text-[11px] text-muted-foreground">{platform.description}</span>{selected && <Check className="absolute end-2 top-2 size-3.5 text-accent-text" aria-hidden="true" />}</button> })}</div></div>
          {accountPlatform === "openai" && <div><p className="text-sm font-medium">认证方式</p><div className="mt-2 grid grid-cols-2 gap-2"><button type="button" onClick={() => { setAccountMethod("oauth"); setAccountError("") }} className={cn("rounded-xl border p-3 text-start", accountMethod === "oauth" ? "border-accent-solid/40 bg-accent-subtle" : "border-border-default bg-bg-surface")}><span className="text-sm font-semibold">ChatGPT / Codex OAuth</span><span className="mt-1 block text-xs text-muted-foreground">使用订阅账户授权</span></button><button type="button" onClick={() => { setAccountMethod("api_key"); setAccountError("") }} className={cn("rounded-xl border p-3 text-start", accountMethod === "api_key" ? "border-accent-solid/40 bg-accent-subtle" : "border-border-default bg-bg-surface")}><span className="text-sm font-semibold">OpenAI API Key</span><span className="mt-1 block text-xs text-muted-foreground">使用平台 API Key</span></button></div></div>}
          <Field id="account-name" label="账户名称"><Input id="account-name" value={accountName} onChange={event => setAccountName(event.target.value)} placeholder="例如：我的 OpenAI" required /></Field>
          {accountPlatform === "openai" && accountMethod === "oauth" ? <div className="space-y-4"><Button type="button" variant="outline" className="w-full" onClick={startOAuth} disabled={oauthStarting}>{oauthStarting && <RotateCw className="animate-spin" aria-hidden="true" />}生成并打开授权链接<ExternalLink aria-hidden="true" /></Button>{oauthStart && <><a href={oauthStart.auth_url} target="_blank" rel="noreferrer" className="inline-flex items-center gap-2 text-sm font-medium text-accent-text hover:underline">重新打开授权页面<ExternalLink className="size-4" aria-hidden="true" /></a><Field id="oauth-result" label="授权结果"><Input id="oauth-result" value={oauthCode} onChange={event => { setOauthCode(event.target.value); setAccountError("") }} placeholder="粘贴回调地址或授权码" autoComplete="off" /></Field></>}</div> : <><Field id="account-api-key" label="API Key"><Input id="account-api-key" type="password" autoComplete="off" spellCheck={false} value={accountKey} onChange={event => { setAccountKey(event.target.value); setAccountError("") }} placeholder="输入平台 API Key" required /></Field><Field id="account-base-url" label="Base URL"><Input id="account-base-url" type="url" inputMode="url" spellCheck={false} value={accountBaseUrl} onChange={event => setAccountBaseUrl(event.target.value)} placeholder="https://api.example.com/v1" required /></Field></>}
          <FormError message={accountError} />
          <div className="flex gap-3"><Button type="button" variant="outline" className="flex-1" onClick={() => setAccountOpen(false)}>取消</Button><Button type="submit" className="flex-1" disabled={accountSubmitting || (accountPlatform === "openai" && accountMethod === "oauth" && !oauthStart)}>{accountSubmitting && <RotateCw className="animate-spin" aria-hidden="true" />}{accountPlatform === "openai" && accountMethod === "oauth" ? "完成授权" : "创建账户"}</Button></div>
        </form>
      </FormDialog>

      <FormDialog open={dialogOpen} onOpenChange={setDialogOpen} title={editTarget ? "编辑供应商" : "添加供应商"} description={editTarget ? "更新连接地址或替换上游凭证。" : "选择服务类型后，只需填写名称和 API Key。"}>
        <form onSubmit={saveProvider} className="space-y-5" aria-busy={submitting}>
          <div><p className="text-sm font-medium">服务类型</p><div className="mt-2 grid grid-cols-3 gap-2">{providerTemplates.map(template => { const Icon = template.icon; const selected = form.provider_type === template.id; return <button key={template.id} type="button" aria-pressed={selected} onClick={() => selectTemplate(template)} className={cn("relative min-h-24 rounded-xl border p-3 text-start transition-colors", selected ? "border-accent-solid/40 bg-accent-subtle" : "border-border-default bg-bg-surface hover:border-border-strong hover:bg-bg-sunken")}><Icon className={cn("size-4", selected ? "text-accent-text" : "text-muted-foreground")} aria-hidden="true" /><span className="mt-2 block text-xs font-semibold">{template.label}</span><span className="mt-1 hidden text-[11px] leading-4 text-muted-foreground sm:block">{template.description}</span>{selected && <Check className="absolute end-2 top-2 size-3.5 text-accent-text" aria-hidden="true" />}</button> })}</div></div>
          <Field id="provider-name" label="名称"><Input id="provider-name" name="provider-name" value={form.name} onChange={event => setForm({ ...form, name: event.target.value })} placeholder="例如：本地 Sub2API" required /></Field>
          <Field id="provider-url" label="Base URL"><Input id="provider-url" name="provider-url" type="url" inputMode="url" spellCheck={false} value={form.base_url} onChange={event => setForm({ ...form, base_url: event.target.value })} placeholder="https://api.example.com/v1" required /></Field>
          <Field id="provider-key" label={form.provider_type === "sub2api" ? "Sub2API API Key" : "上游 API Key"} hint={editTarget ? "留空保持现有凭证" : form.provider_type === "openai-compatible" ? "可选" : undefined}><Input id="provider-key" name="provider-key" type="password" autoComplete="off" spellCheck={false} value={form.api_key} onChange={event => setForm({ ...form, api_key: event.target.value })} placeholder={editTarget ? "不更改可留空" : "输入上游 API Key"} required={!editTarget && form.provider_type !== "openai-compatible"} /></Field>
          <FormError message={formError} />
          <div className="flex gap-3 pt-1"><Button type="button" variant="outline" className="flex-1" onClick={() => setDialogOpen(false)}>取消</Button><Button type="submit" className="flex-1" disabled={submitting}>{submitting && <RotateCw className="animate-spin" aria-hidden="true" />}{editTarget ? "保存" : "添加并同步"}</Button></div>
        </form>
      </FormDialog>

      <FormDialog open={Boolean(syncTarget)} onOpenChange={open => { if (!open) setSyncTarget(null) }} title={`同步模型${syncTarget ? ` · ${syncTarget.name}` : ""}`} description="从上游读取模型，选中后以相同名称创建路由别名。">
        <form onSubmit={importModels}>
          {syncLoading ? <div className="flex min-h-48 items-center justify-center gap-2 text-sm text-muted-foreground"><RotateCw className="size-4 animate-spin" aria-hidden="true" />正在读取模型</div> : syncModels.length > 0 ? <><div className="flex items-center justify-between gap-3"><span className="text-xs text-muted-foreground">已选 {selectedModels.length} / {syncModels.length}</span><Button type="button" variant="ghost" size="sm" onClick={() => setSelectedModels(selectedModels.length === syncModels.length ? [] : syncModels)}>{selectedModels.length === syncModels.length ? "取消全选" : "全选"}</Button></div><div className="mt-3 max-h-64 space-y-1 overflow-y-auto rounded-xl border border-[var(--glass-border)] bg-[var(--glass-bg-subtle)] p-2">{syncModels.map(model => <label key={model} className="flex min-h-10 cursor-pointer items-center gap-3 rounded-lg px-3 py-2 text-sm hover:bg-bg-surface"><input type="checkbox" checked={selectedModels.includes(model)} onChange={() => toggleModel(model)} className="size-4 accent-[var(--color-accent-solid)]" /><span className="min-w-0 break-all font-mono text-xs">{model}</span></label>)}</div></> : !syncError && <div className="flex min-h-48 items-center justify-center text-sm text-muted-foreground">没有可用模型</div>}
          <FormError message={syncError} />
          <div className="mt-5 flex gap-3"><Button type="button" variant="outline" className="flex-1" onClick={() => setSyncTarget(null)}>取消</Button><Button type="submit" className="flex-1" disabled={syncLoading || syncSubmitting || !selectedModels.length}>{syncSubmitting && <RotateCw className="animate-spin" aria-hidden="true" />}导入模型</Button></div>
        </form>
      </FormDialog>

      <ConfirmDialog open={Boolean(deleteTarget)} onOpenChange={open => { if (!open) setDeleteTarget(null) }} title={`删除“${deleteTarget?.name || ""}”？`} description="删除后无法恢复，使用它的模型路由将无法继续调用。" confirmText="确认删除" cancelText="保留" destructive onConfirm={remove} />
    </div>
  )
}

function parseOAuthResult(value: string, fallbackState: string) {
  const input = value.trim()
  if (!input) return { code: "", state: fallbackState }
  try {
    const url = new URL(input)
    return { code: url.searchParams.get("code") || "", state: url.searchParams.get("state") || fallbackState }
  } catch {
    return { code: input, state: fallbackState }
  }
}

function providerLabel(type: string) {
  return providerTemplates.find(item => item.id === type)?.label || type
}
