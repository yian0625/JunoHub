"use client"

import { useEffect, useState } from "react"
import { RotateCw } from "lucide-react"
import { consoleAPI, ConsoleProviderAccount } from "@/lib/api"
import { toast } from "sonner"
import { AppForm } from "@/components/ui/app-form"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"

export function AccountDispatchEditor({ providerId, accounts, onSaved, onDirtyChange, onBusyChange }: { providerId: number; accounts: ConsoleProviderAccount[]; onSaved: (accounts: ConsoleProviderAccount[]) => void; onDirtyChange: (dirty: boolean) => void; onBusyChange: (busy: boolean) => void }) {
  const [expanded, setExpanded] = useState<number[]>([])
  const initialDraft = accounts.map(account => ({ id: account.id, rpm: String(account.rpm || 0), models: (account.allowed_models || []).join(", "), priority: String(account.priority ?? 1), weight: String(account.weight || 1), concurrency: String(account.concurrency || 1) }))
  const [draft, setDraft] = useState(initialDraft)
  const dirty = JSON.stringify(draft) !== JSON.stringify(initialDraft)
  useEffect(() => { onDirtyChange(dirty) }, [dirty, onDirtyChange])
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const update = (id: number, field: "priority" | "weight" | "concurrency" | "rpm", value: string) => { if (/^\d{0,7}$/.test(value)) { setDraft(current => current.map(a => a.id === id ? { ...a, [field]: field === "rpm" ? value || "0" : value } : a)); setError("") } }
  const save = async (event: React.FormEvent) => {
    event.preventDefault()
    const updates = draft.map(a => ({ id: a.id, member_order: accounts.find(source => source.id === a.id)?.member_order ?? 0, rpm: Number(a.rpm), priority: Number(a.priority), weight: Number(a.weight), concurrency: Number(a.concurrency), allowed_models: [...new Set(a.models.split(/[,，\n]/).map(model => model.trim()).filter(Boolean))] }))
    if (updates.some(a => a.priority < 0 || a.priority > 999 || a.weight < 1 || a.weight > 1000 || a.concurrency < 1 || a.concurrency > 1000 || a.rpm < 0 || a.rpm > 1000000 || !draft.find(d => d.id === a.id)?.rpm)) { setError("分组填写 1–999，权重和并发填写 1–1000，RPM 填写 0–1000000。"); return }
    setBusy(true); onBusyChange(true); setError("")
    try { await consoleAPI.providerAccountDispatch(providerId, updates); onSaved(accounts.map(a => ({ ...a, ...updates.find(update => update.id === a.id) })).sort((a,b) => a.priority - b.priority || a.id - b.id)); toast.success("账户容量已保存") }
    catch (error) { setError(error instanceof Error ? error.message : "保存失败，请重试。") }
    finally { setBusy(false); onBusyChange(false) }
  }
  if (!accounts.length) return <p className="py-8 text-center text-sm text-muted-foreground">先添加账户，再配置容量。</p>
  return <AppForm onSubmit={save} className="space-y-5" aria-busy={busy}>
    <div><h3 className="text-base font-semibold">每个账户可以承担多少请求？</h3><p className="mt-2 text-xs leading-5 text-muted-foreground">这里只设置每个账户能承担的请求量。主用顺序和分配份额在“请求分配”中一起配置。</p></div>
    <fieldset disabled={busy} className="space-y-3"><legend className="sr-only">账户分组与容量</legend>
      {draft.map(account => { const source = accounts.find(a => a.id === account.id)!; return <section key={account.id} className="rounded-xl border border-border-default p-4 sm:p-5">
        <div className="flex flex-wrap items-baseline justify-between gap-2"><h4 className="text-sm font-semibold [overflow-wrap:anywhere]">{source.name}</h4></div>
        <div className="mt-4 grid grid-cols-2 gap-3 sm:grid-cols-2">{([{ field: "concurrency", label: "并发上限" },{ field: "rpm", label: "每分钟请求数" }] as const).map(({field,label}) => <div key={field} className="space-y-2"><Label htmlFor={`account-${account.id}-${field}`} className="text-xs">{label}</Label><Input id={`account-${account.id}-${field}`} aria-label={`${source.name} ${label}`} inputMode="numeric" placeholder={field === "rpm" ? "不限" : undefined} value={field === "rpm" && account.rpm === "0" ? "" : account[field]} onChange={event => update(account.id,field,event.target.value)} className="px-3 tabular-nums" required={field !== "rpm"} /></div>)}</div>
        <button type="button" aria-expanded={expanded.includes(account.id)} className="mt-4 rounded text-xs font-medium text-accent-text focus-visible:outline-2 focus-visible:outline-ring" onClick={() => setExpanded(current => current.includes(account.id) ? current.filter(id => id !== account.id) : [...current, account.id])}>{expanded.includes(account.id) ? "收起模型范围" : `模型范围 · ${account.models.trim() ? "已指定" : "不限制"}`}</button>
        <div hidden={!expanded.includes(account.id)} className="mt-3 space-y-2"><Label htmlFor={`account-${account.id}-models`} className="text-xs">允许使用的模型</Label><Input id={`account-${account.id}-models`} aria-label={`${source.name} 允许使用的模型`} value={account.models} onChange={event => setDraft(current => current.map(a => a.id === account.id ? { ...a, models: event.target.value } : a))} placeholder="留空不限；多个模型用逗号分隔" spellCheck={false} /></div>
      </section> })}
    </fieldset>
    <p className="text-xs leading-5 text-muted-foreground">并发是同时处理的请求数；每分钟请求数留空表示不限。</p>
    {error && <p role="alert" className="rounded-xl bg-danger-subtle p-3 text-sm text-danger-text">{error}</p>}
    <div className="flex flex-wrap items-center justify-between gap-3 border-t border-border-default pt-4"><p className="text-xs text-muted-foreground">{dirty ? "修改尚未生效" : "与已保存配置一致"}</p><div className="flex gap-2"><Button type="button" variant="outline" disabled={!dirty || busy} onClick={() => setDraft(initialDraft)}>撤销修改</Button><Button type="submit" disabled={busy || !dirty}>{busy && <RotateCw className="animate-spin" aria-hidden="true" />}保存账户容量</Button></div></div>
  </AppForm>
}
