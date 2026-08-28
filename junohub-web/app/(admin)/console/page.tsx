"use client"

import { useEffect, useMemo, useState } from "react"
import { useRouter } from "next/navigation"
import { Activity, Check, CheckCircle2, CircleDashed, Copy, KeyRound, Network, Plus, RotateCw, Server, ShieldCheck, Trash2, Zap } from "lucide-react"
import { Button, buttonVariants } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { ConfirmDialog } from "@/components/confirm-dialog"
import { cn } from "@/lib/utils"
import { ConsoleKey, ConsoleModel, ConsoleProvider, consoleAPI } from "@/lib/api"
import { useAuthStore } from "@/lib/store"
import { toast } from "sonner"

type DeleteTarget = { kind: "provider" | "model" | "key"; id: number; name: string }

export default function ConsolePage() {
  const router = useRouter()
  const user = useAuthStore(state => state.user)
  const [providers, setProviders] = useState<ConsoleProvider[]>([])
  const [models, setModels] = useState<ConsoleModel[]>([])
  const [keys, setKeys] = useState<ConsoleKey[]>([])
  const [stats, setStats] = useState({ providers: 0, models: 0, keys: 0, requests: 0 })
  const [createdKey, setCreatedKey] = useState("")
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState("")
  const [formError, setFormError] = useState({ provider: "", model: "", key: "" })
  const [submitting, setSubmitting] = useState<"provider" | "model" | "key" | "">("")
  const [testingId, setTestingId] = useState<number | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<DeleteTarget | null>(null)
  const [providerForm, setProviderForm] = useState({ name: "", provider_type: "openai-compatible", base_url: "https://api.openai.com/v1", api_key: "" })
  const [modelForm, setModelForm] = useState({ provider_id: "", alias: "", upstream_model: "" })
  const [keyName, setKeyName] = useState("默认密钥")
  const baseURL = useMemo(() => typeof window === "undefined" ? "/v1" : `${window.location.origin}/v1`, [])

  const load = async () => {
    setLoading(true); setLoadError("")
    try {
      const [providerData, modelData, keyData, statData] = await Promise.all([consoleAPI.providerList(), consoleAPI.modelList(), consoleAPI.keyList(), consoleAPI.stats()])
      setProviders(providerData.list || []); setModels(modelData.list || []); setKeys(keyData.list || []); setStats(statData)
      setModelForm(current => ({ ...current, provider_id: current.provider_id || String(providerData.list?.[0]?.id || "") }))
    } catch (error) {
      const message = error instanceof Error ? error.message : "无法加载控制台，请检查网络后重试。"
      setLoadError(message); toast.error(message)
    } finally { setLoading(false) }
  }

  useEffect(() => {
    if (user && user.role === "admin") { router.replace("/juno-hub"); return }
    if (user?.role === "user") load()
  }, [user, router])

  const addProvider = async (event: React.FormEvent) => {
    event.preventDefault(); setFormError(current => ({ ...current, provider: "" })); setSubmitting("provider")
    try {
      await consoleAPI.providerCreate(providerForm)
      setProviderForm(current => ({ ...current, name: "", api_key: "" })); await load(); toast.success("供应商已添加")
    } catch (error) { setFormError(current => ({ ...current, provider: error instanceof Error ? error.message : "无法添加供应商，请检查填写内容后重试。" })) }
    finally { setSubmitting("") }
  }

  const addModel = async (event: React.FormEvent) => {
    event.preventDefault(); setFormError(current => ({ ...current, model: "" })); setSubmitting("model")
    try {
      await consoleAPI.modelCreate({ provider_id: Number(modelForm.provider_id), alias: modelForm.alias.trim(), upstream_model: modelForm.upstream_model.trim(), capabilities: ["chat"] })
      setModelForm(current => ({ ...current, alias: "", upstream_model: "" })); await load(); toast.success("模型路由已添加")
    } catch (error) { setFormError(current => ({ ...current, model: error instanceof Error ? error.message : "无法添加模型路由，请检查填写内容后重试。" })) }
    finally { setSubmitting("") }
  }

  const createKey = async (event: React.FormEvent) => {
    event.preventDefault(); setFormError(current => ({ ...current, key: "" })); setSubmitting("key")
    try {
      const key = await consoleAPI.keyCreate({ name: keyName.trim() }); setCreatedKey(key.key || ""); await load(); toast.success("访问密钥已创建")
    } catch (error) { setFormError(current => ({ ...current, key: error instanceof Error ? error.message : "无法创建密钥，请重试。" })) }
    finally { setSubmitting("") }
  }

  const copy = async (text: string, label: string) => { await navigator.clipboard.writeText(text); toast.success(`${label}已复制`) }
  const testProvider = async (item: ConsoleProvider) => {
    setTestingId(item.id)
    try {
      const result = await consoleAPI.providerTest(item.id)
      if (result.ok) toast.success(`${item.name} 连接正常，HTTP ${result.status_code}`)
      else toast.error(`${item.name} 连接失败，HTTP ${result.status_code}`)
    }
    catch { toast.error(`无法测试 ${item.name}，请检查地址和凭证后重试。`) } finally { setTestingId(null) }
  }
  const remove = async () => {
    if (!deleteTarget) return
    const { kind, id } = deleteTarget
    try {
      if (kind === "provider") await consoleAPI.providerDelete(id)
      if (kind === "model") await consoleAPI.modelDelete(id)
      if (kind === "key") await consoleAPI.keyDelete(id)
      setDeleteTarget(null); await load(); toast.success("已删除")
    } catch (error) { toast.error(error instanceof Error ? error.message : "无法删除，请稍后重试。") }
  }

  if (loading && !providers.length && !models.length && !keys.length) return <LoadingState />
  if (loadError && !providers.length && !models.length && !keys.length) return <ErrorState message={loadError} onRetry={load} />

  const metrics = [
    { label: "供应商", value: stats.providers, note: "上游连接", icon: Server },
    { label: "模型路由", value: stats.models, note: "可调用模型", icon: Network },
    { label: "访问密钥", value: stats.keys, note: "有效凭证", icon: KeyRound },
    { label: "累计请求", value: stats.requests, note: "网关调用", icon: Activity },
  ]
  const setup = [providers.length > 0, models.length > 0, keys.length > 0]

  return (
    <div className="view-enter space-y-14">
      <header className="flex flex-col gap-7 border-b border-border-subtle pb-10 xl:flex-row xl:items-end xl:justify-between">
        <div><p className="eyebrow mb-3">个人工作区</p><h1 className="page-heading">模型网关</h1><p className="page-lead mt-3">集中管理上游服务、模型别名和访问密钥。所有请求通过一个 OpenAI 兼容入口转发。</p></div>
        <a href="#providers" className={cn(buttonVariants({ size: "lg" }), "w-fit")}><Plus aria-hidden="true" />添加供应商</a>
      </header>

      <section aria-labelledby="overview-title">
        <h2 id="overview-title" className="sr-only">网关概览</h2>
        <div className="grid grid-cols-2 overflow-hidden rounded-xl border border-border-subtle bg-bg-surface lg:grid-cols-4">
          {metrics.map(metric => <MetricCard key={metric.label} {...metric} />)}
        </div>
      </section>

      <section className="grid gap-4 xl:grid-cols-[minmax(0,1.4fr)_minmax(320px,.6fr)]" aria-label="快速开始">
        <Card className="gap-0 py-0"><CardContent className="p-5 sm:p-6"><div className="flex flex-col gap-5 sm:flex-row sm:items-center sm:justify-between"><div><p className="text-sm font-semibold">统一请求地址</p><p className="mt-1 text-sm text-muted-foreground">在任何 OpenAI 兼容客户端中使用</p></div><span className="inline-flex w-fit items-center gap-2 rounded-full bg-success-subtle px-3 py-1.5 text-xs font-medium text-success-text"><CheckCircle2 className="size-3.5" aria-hidden="true" />服务可用</span></div><div className="mt-5 flex items-center gap-2 rounded-xl bg-bg-sunken p-2 ps-3 shadow-[inset_0_0_0_1px_var(--color-border-subtle)]"><code className="min-w-0 flex-1 break-all text-sm text-foreground"><bdi>{baseURL}</bdi></code><Button variant="outline" size="icon" onClick={() => copy(baseURL, "请求地址")} aria-label="复制统一请求地址"><Copy aria-hidden="true" /></Button></div></CardContent></Card>
        <Card className="gap-0 py-0"><CardContent className="p-5 sm:p-6"><div className="flex items-center justify-between"><div><p className="text-sm font-semibold">配置进度</p><p className="mt-1 text-sm text-muted-foreground">完成三步即可发起请求</p></div><span className="metric-number text-xl">{setup.filter(Boolean).length}/3</span></div><ol className="mt-5 space-y-3">{["连接供应商", "添加模型路由", "创建访问密钥"].map((label, index) => <li key={label} className="flex items-center gap-3 text-sm"><span className={cn("grid size-6 place-items-center rounded-full", setup[index] ? "bg-success-subtle text-success-text" : "bg-bg-sunken text-muted-foreground")}>{setup[index] ? <Check className="size-3.5" aria-hidden="true" /> : <span className="text-xs tabular-nums">{index + 1}</span>}</span><span className={setup[index] ? "text-foreground" : "text-muted-foreground"}>{label}</span></li>)}</ol></CardContent></Card>
      </section>

      <ResourceSection id="providers" icon={ShieldCheck} title="供应商" description="连接 OpenAI 兼容服务或本地模型节点。">
        <ResourceList emptyTitle="还没有供应商" emptyDescription="添加第一个上游服务后，才能创建模型路由。" items={providers.map(item => ({ id: item.id, title: item.name, detail: `${item.provider_type} · ${item.base_url}`, active: item.enabled === 1, actions: <Button size="sm" variant="outline" onClick={() => testProvider(item)} disabled={testingId === item.id}>{testingId === item.id && <RotateCw className="animate-spin" aria-hidden="true" />}测试连接</Button>, onDelete: () => setDeleteTarget({ kind: "provider", id: item.id, name: item.name }) }))} />
        <FormCard title="添加供应商" description="凭证仅保存在你的私有部署中。"><form onSubmit={addProvider} className="space-y-5" aria-busy={submitting === "provider"}><Field id="provider-name" label="名称"><Input id="provider-name" name="provider-name" value={providerForm.name} onChange={event => setProviderForm({ ...providerForm, name: event.target.value })} placeholder="例如：生产环境 OpenAI" required /></Field><Field id="provider-protocol" label="协议类型"><Input id="provider-protocol" name="provider-protocol" value={providerForm.provider_type} onChange={event => setProviderForm({ ...providerForm, provider_type: event.target.value })} required /></Field><Field id="provider-url" label="Base URL"><Input id="provider-url" name="provider-url" type="url" inputMode="url" spellCheck={false} value={providerForm.base_url} onChange={event => setProviderForm({ ...providerForm, base_url: event.target.value })} placeholder="https://api.example.com/v1" required /></Field><Field id="provider-key" label="上游 API Key" hint="可选"><Input id="provider-key" name="provider-key" type="password" autoComplete="off" spellCheck={false} value={providerForm.api_key} onChange={event => setProviderForm({ ...providerForm, api_key: event.target.value })} placeholder="无鉴权服务可留空" /></Field><FormError message={formError.provider} /><Button type="submit" className="w-full" disabled={submitting === "provider"}>{submitting === "provider" && <RotateCw className="animate-spin" aria-hidden="true" />}添加供应商</Button></form></FormCard>
      </ResourceSection>

      <ResourceSection id="models" icon={Network} title="模型路由" description="为上游模型设置稳定、易记的对外名称。">
        <ResourceList emptyTitle="还没有模型路由" emptyDescription="先添加供应商，再把上游模型映射为对外别名。" items={models.map(item => ({ id: item.id, title: item.alias, detail: `${item.provider_name || item.provider_type} → ${item.upstream_model}`, active: item.enabled === 1, onDelete: () => setDeleteTarget({ kind: "model", id: item.id, name: item.alias }) }))} />
        <FormCard title="添加模型路由" description="客户端只需要使用你设置的别名。"><form onSubmit={addModel} className="space-y-5" aria-busy={submitting === "model"}><Field id="model-provider" label="供应商"><select id="model-provider" name="model-provider" className="field-control w-full px-3.5 text-base sm:text-sm" value={modelForm.provider_id} onChange={event => setModelForm({ ...modelForm, provider_id: event.target.value })} required><option value="">选择供应商</option>{providers.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select></Field><Field id="model-alias" label="对外别名"><Input id="model-alias" name="model-alias" spellCheck={false} value={modelForm.alias} onChange={event => setModelForm({ ...modelForm, alias: event.target.value })} placeholder="例如：fast-chat" required /></Field><Field id="upstream-model" label="上游模型名称"><Input id="upstream-model" name="upstream-model" spellCheck={false} value={modelForm.upstream_model} onChange={event => setModelForm({ ...modelForm, upstream_model: event.target.value })} placeholder="例如：gpt-4o" required /></Field><FormError message={formError.model} /><Button type="submit" className="w-full" aria-disabled={!providers.length || undefined} onClick={event => { if (!providers.length) { event.preventDefault(); setFormError(current => ({ ...current, model: "请先添加供应商，再创建模型路由。" })) } }} disabled={submitting === "model"}>{submitting === "model" && <RotateCw className="animate-spin" aria-hidden="true" />}添加模型路由</Button></form></FormCard>
      </ResourceSection>

      <ResourceSection id="keys" icon={KeyRound} title="访问密钥" description="为不同应用签发独立凭证，方便单独撤销。">
        <div className="space-y-4"><ResourceList emptyTitle="还没有访问密钥" emptyDescription="创建密钥后，客户端才能调用网关。" items={keys.map(item => ({ id: item.id, title: item.name, detail: `${item.key_masked} · ${item.total_requests || 0} 次请求`, active: item.status === 1, onDelete: () => setDeleteTarget({ kind: "key", id: item.id, name: item.name }) }))} />{createdKey && <Card className="bg-warning-subtle shadow-[inset_0_0_0_1px_var(--color-warning-solid)]"><CardContent><div className="flex gap-3"><KeyRound className="mt-0.5 size-5 shrink-0 text-warning-text" aria-hidden="true" /><div className="min-w-0 flex-1"><p className="font-semibold text-warning-text">立即保存新密钥</p><p className="mt-1 text-sm leading-6 text-warning-text">出于安全考虑，完整密钥只显示这一次。</p><div className="mt-4 flex items-center gap-2 rounded-xl bg-bg-surface p-2 ps-3"><code className="min-w-0 flex-1 break-all text-sm"><bdi>{createdKey}</bdi></code><Button variant="outline" size="icon" onClick={() => copy(createdKey, "访问密钥")} aria-label="复制新访问密钥"><Copy aria-hidden="true" /></Button></div></div></div></CardContent></Card>}</div>
        <FormCard title="创建访问密钥" description="建议按应用或设备命名。"><form onSubmit={createKey} className="space-y-5" aria-busy={submitting === "key"}><Field id="key-name" label="密钥名称"><Input id="key-name" name="key-name" value={keyName} onChange={event => setKeyName(event.target.value)} placeholder="例如：桌面客户端" required /></Field><FormError message={formError.key} /><Button type="submit" className="w-full" disabled={submitting === "key"}>{submitting === "key" && <RotateCw className="animate-spin" aria-hidden="true" />}创建访问密钥</Button></form></FormCard>
      </ResourceSection>

      <ConfirmDialog open={Boolean(deleteTarget)} onOpenChange={open => { if (!open) setDeleteTarget(null) }} title={`删除“${deleteTarget?.name || ""}”？`} description="删除后无法恢复，使用它的现有请求可能立即失败。" confirmText="确认删除" cancelText="保留" destructive onConfirm={remove} />
    </div>
  )
}

function MetricCard({ label, value, note, icon: Icon }: { label: string; value: number; note: string; icon: React.ElementType }) {
  return <div className="border-border-subtle p-5 odd:border-e lg:border-e lg:last:border-e-0"><div className="flex items-center justify-between gap-3"><p className="text-xs font-medium text-muted-foreground">{label}</p><Icon className="size-4 text-text-disabled" strokeWidth={1.5} aria-hidden="true" /></div><p className="mt-5 font-mono text-3xl font-medium tracking-[-.06em] tabular-nums">{value}</p><p className="mt-1 text-[11px] text-muted-foreground">{note}</p></div>
}

function ResourceSection({ id, icon: Icon, title, description, children }: { id: string; icon: React.ElementType; title: string; description: string; children: React.ReactNode }) {
  const content = Array.isArray(children) ? children : [children]
  return <section id={id} className="section-anchor space-y-6 border-t border-border-subtle pt-10" aria-labelledby={`${id}-title`}><div className="flex items-start gap-4"><span className="mt-1 grid size-7 shrink-0 place-items-center rounded-md bg-accent-solid text-text-inverse"><Icon className="size-3.5" strokeWidth={2} aria-hidden="true" /></span><div><h2 id={`${id}-title`} className="text-xl font-semibold leading-snug tracking-[-.025em]">{title}</h2><p className="mt-1 text-sm leading-6 text-muted-foreground">{description}</p></div></div><div className="grid items-start gap-5 xl:grid-cols-[minmax(0,1.6fr)_minmax(320px,.4fr)]"><div>{content[0]}</div><div>{content[1]}</div></div></section>
}

function FormCard({ title, description, children }: { title: string; description: string; children: React.ReactNode }) {
  return <Card className="bg-bg-sunken shadow-none"><CardHeader><CardTitle>{title}</CardTitle><CardDescription>{description}</CardDescription></CardHeader><CardContent>{children}</CardContent></Card>
}

function Field({ id, label, hint, children }: { id: string; label: string; hint?: string; children: React.ReactNode }) {
  return <div className="space-y-2"><div className="flex items-center justify-between gap-3"><Label htmlFor={id} className="text-sm font-medium">{label}</Label>{hint && <span className="text-xs text-muted-foreground">{hint}</span>}</div>{children}</div>
}

function FormError({ message }: { message: string }) { return message ? <p role="alert" className="rounded-xl bg-danger-subtle px-3 py-2.5 text-sm leading-5 text-danger-text">{message}</p> : null }

function ResourceList({ items, emptyTitle, emptyDescription }: { items: Array<{ id: number; title: string; detail: string; active: boolean; onDelete: () => void; actions?: React.ReactNode }>; emptyTitle: string; emptyDescription: string }) {
  if (!items.length) return <div className="flex min-h-52 flex-col items-start justify-center rounded-xl border border-dashed border-border-default bg-bg-surface p-8"><CircleDashed className="size-6 text-text-disabled" strokeWidth={1.5} aria-hidden="true" /><p className="mt-5 text-sm font-semibold">{emptyTitle}</p><p className="mt-1 max-w-sm text-sm leading-6 text-muted-foreground">{emptyDescription}</p></div>
  return <div className="overflow-hidden rounded-xl border border-border-subtle bg-bg-surface"><div className="divide-y divide-border-subtle">{items.map(item => <article key={item.id} className="flex flex-col gap-4 p-4 transition-colors duration-150 hover:bg-bg-raised sm:flex-row sm:items-center sm:justify-between sm:p-5"><div className="flex min-w-0 items-start gap-3"><span className={cn("mt-1 size-2 shrink-0 rounded-full", item.active ? "bg-success-solid" : "bg-text-disabled")} aria-hidden="true" /><div className="min-w-0"><h3 className="break-words text-sm font-semibold">{item.title}</h3><p className="mt-1 break-all font-mono text-xs leading-5 text-muted-foreground">{item.detail}</p><p className="mt-1 text-[11px] text-muted-foreground">{item.active ? "已启用" : "已停用"}</p></div></div><div className="flex shrink-0 items-center gap-3 ps-5 sm:ps-0">{item.actions}<Button size="icon" variant="ghost" onClick={item.onDelete} className="text-muted-foreground hover:bg-danger-subtle hover:text-danger-text" aria-label={`删除 ${item.title}`}><Trash2 aria-hidden="true" /></Button></div></article>)}</div></div>
}

function LoadingState() { return <div className="flex min-h-[60vh] flex-col items-center justify-center" role="status"><RotateCw className="size-6 animate-spin text-accent-text" aria-hidden="true" /><p className="mt-4 text-sm font-medium">正在加载网关数据…</p><p className="mt-1 text-sm text-muted-foreground">这通常只需要几秒钟。</p></div> }
function ErrorState({ message, onRetry }: { message: string; onRetry: () => void }) { return <div className="mx-auto flex min-h-[60vh] max-w-md flex-col items-center justify-center text-center" role="alert"><span className="grid size-12 place-items-center rounded-2xl bg-danger-subtle text-danger-text"><Zap className="size-5" aria-hidden="true" /></span><h1 className="mt-5 text-xl font-semibold">无法打开控制台</h1><p className="mt-2 text-sm leading-6 text-muted-foreground">{message}</p><Button className="mt-6" onClick={onRetry}><RotateCw aria-hidden="true" />重新加载</Button></div> }
