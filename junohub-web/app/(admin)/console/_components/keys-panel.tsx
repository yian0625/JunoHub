"use client"

import { RequestLimitsFields, useRequestLimits } from "@/components/resource-limits-fields"
import { Textarea } from "@/components/coss-ui/textarea"
import { AppForm } from "@/components/ui/app-form"
import { DataTable, type Column } from "@/components/data-table"

import { useCallback, useEffect, useState } from "react"
import { Copy, Eye, Pencil, MoreHorizontal, SearchIcon, Trash2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Button as CossButton } from "@/components/coss-ui/button"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/coss-ui/input-group"
import { Input as CossInput } from "@/components/coss-ui/input"
import { AppTooltip } from "@/components/ui/tooltip"
import { SelectField } from "@/components/ui/select-field"
import { DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem } from "@/components/ui/dropdown-menu"
import { Input } from "@/components/ui/input"
import { ConfirmDialog } from "@/components/confirm-dialog"
import { ConsoleKey, consoleAPI } from "@/lib/api"
import { toast } from "sonner"
import { EmptyGrid, ErrorState, Field, FormDialog, FormError, LoadingState, PageHead, NoSearchResults } from "./console-ui"
import { useConsoleGuard } from "./use-console-guard"

export function KeysPanel({ ownOnly = false }: { ownOnly?: boolean }) {
  const isUser = useConsoleGuard()
  const [query, setQuery] = useState("")
  const [page, setPage] = useState(1)
  const [scope, setScope] = useState<"all" | "own">("own")
  const [keys, setKeys] = useState<ConsoleKey[]>([])
  const [createdKey, setCreatedKey] = useState("")
  const [keyLabel, setKeyLabel] = useState("")
  const [editing, setEditing] = useState<ConsoleKey | null>(null)
  const [status, setStatus] = useState("1")
  const [revealing, setRevealing] = useState<number | null>(null)
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState("")
  const [formError, setFormError] = useState("")
  const [submitting, setSubmitting] = useState(false)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<{ id: number; name: string } | null>(null)
  const [modelScope, setModelScope] = useState("all")
  const [modelNames, setModelNames] = useState("")
  const [expiry, setExpiry] = useState("never")
  const [name, setName] = useState("默认密钥")
  const limits = useRequestLimits("key", editing?.id ?? null, dialogOpen)
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000))
  useEffect(() => { const timer = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 30_000); return () => clearInterval(timer) }, [])

  const load = useCallback(async () => {
    setLoading(true); setLoadError("")
    try { const data = ownOnly ? await consoleAPI.keyOwnList() : await consoleAPI.keyList(); setKeys(data.list || []); setScope(data.scope || "own") }
    catch (error) { const message = error instanceof Error ? error.message : "无法加载访问密钥，请检查网络后重试。"; setLoadError(message); toast.error(message) }
    finally { setLoading(false) }
  }, [ownOnly])

  useEffect(() => { if (isUser) void load() }, [isUser, load])

  const openDialog = () => { setEditing(null); setModelScope("all"); setModelNames(""); setExpiry("never"); setStatus("1"); setName("默认密钥"); setFormError(""); setDialogOpen(true) }

  const editKey = (item: ConsoleKey) => {
    setEditing(item); setModelScope(item.allowed_models.length ? "custom" : "all"); setModelNames(item.allowed_models.join("\n")); setExpiry("keep"); setName(item.name); setStatus(String(item.status)); setFormError(""); setDialogOpen(true)
  }

  const createKey = async (event: React.FormEvent) => {
    event.preventDefault(); if (submitting) return; setFormError("");
    if (!name.trim()) { setFormError("请输入密钥名称。"); document.getElementById("key-name")?.focus(); return }
    const allowed_models = modelScope === "all" ? [] : [...new Set(modelNames.split(/\n/).map(value => value.trim()).filter(Boolean))]
    if (modelScope === "custom" && !allowed_models.length) { setFormError("请至少填写一个模型别名。"); document.getElementById("key-models")?.focus(); return }
    const expires_at = expiry === "keep" ? editing?.expires_at || 0 : expiry === "never" ? 0 : Math.floor(Date.now() / 1000) + Number(expiry) * 86400
    setSubmitting(true)
    try {
      if (editing) {
        await consoleAPI.keyUpdate({ request_limits: limits.value(), id: editing.id, name: name.trim(), status: Number(status), allowed_models, expires_at })
        setDialogOpen(false); await load(); toast.success("密钥设置已保存")
      } else {
        const key = await consoleAPI.keyCreate({ request_limits: limits.value(), name: name.trim(), allowed_models, expires_at }); setKeyLabel(key.name); setCreatedKey(key.key || ""); setDialogOpen(false); await load(); toast.success("访问密钥已创建")
      }
    } catch (error) { setFormError(error instanceof Error ? error.message : "无法保存密钥，请重试。") }
    finally { setSubmitting(false) }
  }

  const copy = async (text: string, label: string) => { try { await navigator.clipboard.writeText(text); toast.success(`${label}已复制`) } catch { toast.error("复制失败，请选中密钥手动复制。") } }
  const reveal = async (item: ConsoleKey, copyOnly = false) => {
    if (revealing !== null) return
    setRevealing(item.id)
    try { const result = await consoleAPI.keyReveal(item.id); if (copyOnly) await copy(result.key, "访问密钥"); else { setKeyLabel(item.name); setCreatedKey(result.key) } }
    catch (error) { toast.error(error instanceof Error ? error.message : "无法读取密钥，请重试。") }
    finally { setRevealing(null) }
  }

  const remove = async () => {
    if (!deleteTarget) return
    try { await consoleAPI.keyDelete(deleteTarget.id); setDeleteTarget(null); await load(); toast.success("已删除") }
    catch (error) { toast.error(error instanceof Error ? error.message : "无法删除，请稍后重试。") }
  }

  const search = query.trim().toLocaleLowerCase()
  const visibleItems = keys.filter(item => [item.name, item.owner_name, item.owner_email].join(" ").toLocaleLowerCase().includes(search))
  const currentPage = Math.min(page, Math.max(1, Math.ceil(visibleItems.length / 20)))
  const number = (value?: number) => (value || 0).toLocaleString("zh-CN")
  const columns: Column<ConsoleKey>[] = [
    { key: "name", title: "密钥", className: "w-[190px]", render: (_, item) => <div className="whitespace-normal"><p className="font-medium [overflow-wrap:anywhere]">{item.name}</p><p className="mt-1 truncate font-mono text-xs text-muted-foreground">{item.key_masked}</p><p className="mt-1 text-xs text-muted-foreground">{item.allowed_models.length ? `${item.allowed_models.length} 个模型规则` : "全部可用模型"}</p>{!item.can_reveal && <p className="mt-1 text-xs leading-5 text-muted-foreground">旧密钥调用一次后可查看</p>}</div> },
    ...(!ownOnly && scope === "all" ? [{ key: "owner_name", title: "归属用户", className: "w-[175px]", render: (_: unknown, item: ConsoleKey) => <div className="whitespace-normal [overflow-wrap:anywhere]"><p className="font-medium">{item.owner_name || item.owner_email || "未命名用户"}</p>{item.owner_email && <p className="mt-1 text-xs text-muted-foreground">{item.owner_email}</p>}</div> }] : []),
    { key: "status", title: "状态 / 有效期", className: "w-[125px]", render: (_, item) => {
      const expired = item.expires_at > 0 && item.expires_at <= now
      const active = item.status === 1 && !expired
      return <div className="whitespace-normal"><span className={`inline-flex items-center gap-2 ${active ? "text-success-text" : expired && item.status === 1 ? "text-warning-text" : "text-muted-foreground"}`}><span className={`size-1.5 shrink-0 rounded-full ${active ? "bg-success-solid" : expired && item.status === 1 ? "bg-warning-solid" : "bg-muted-foreground/60"}`} aria-hidden="true" />{item.status !== 1 ? "已停用" : expired ? "已过期" : "已启用"}</span><p className="mt-1 text-xs text-muted-foreground">{item.expires_at > 0 ? `到期 ${new Date(item.expires_at * 1000).toLocaleDateString("zh-CN")}` : "不过期"}</p></div>
    } },
    { key: "total_requests", title: "请求数", align: "start", className: "w-[80px] tabular-nums", render: (_, item) => number(item.total_requests) },
    { key: "input_tokens", title: "输入 Token", align: "start", className: "w-[95px] tabular-nums", render: (_, item) => number(item.input_tokens) },
    { key: "output_tokens", title: "输出 Token", align: "start", className: "w-[95px] tabular-nums", render: (_, item) => number(item.output_tokens) },
    { key: "cost_usd", title: "估算消耗（美元）", align: "start", className: "w-[145px] tabular-nums", render: (_, item) => <div><p>{(item.unpriced_requests || 0) > 0 && item.unpriced_requests === item.total_requests ? "待计价" : `$${(item.cost_usd || 0).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 6 })}`}</p>{(item.unpriced_requests || 0) > 0 && <p className="mt-1 text-xs text-muted-foreground">{number(item.unpriced_requests)} 次未计价</p>}</div> },
    { key: "actions", title: "操作", align: "start", className: "w-[120px]", render: (_, item) => <div className="-ms-2 inline-flex items-center gap-0.5">
      <AppTooltip content="查看密钥"><Button size="icon-sm" variant="ghost" aria-label={`查看 ${item.name}`} disabled={!item.can_reveal || revealing !== null} onClick={() => reveal(item)}><Eye className="size-4" aria-hidden="true" /></Button></AppTooltip>
      <AppTooltip content="编辑密钥"><Button size="icon-sm" variant="ghost" aria-label={`编辑 ${item.name}`} onClick={() => editKey(item)}><Pencil className="size-4" aria-hidden="true" /></Button></AppTooltip>
      <DropdownMenu><DropdownMenuTrigger render={<Button size="icon-sm" variant="ghost" aria-label={`${item.name} 的更多操作`} />}><MoreHorizontal className="size-4" aria-hidden="true" /></DropdownMenuTrigger>
        <DropdownMenuContent align="end"><DropdownMenuItem disabled={!item.can_reveal || revealing !== null} onClick={() => reveal(item, true)}><Copy aria-hidden="true" />复制密钥</DropdownMenuItem><DropdownMenuItem variant="destructive" onClick={() => setDeleteTarget({ id: item.id, name: `${item.name}${scope === "all" ? ` · ${item.owner_email || item.owner_name || "未命名用户"}` : ""}` })}><Trash2 aria-hidden="true" />删除密钥</DropdownMenuItem></DropdownMenuContent>
      </DropdownMenu>
    </div> },
  ]

  if (loading && !keys.length) return <LoadingState />
  if (loadError && !keys.length) return <ErrorState message={loadError} onRetry={load} />

  return (
    <div className="view-enter space-y-8">
      <PageHead title={ownOnly ? "我的密钥" : "访问密钥"} description={ownOnly ? "管理当前账户的访问密钥，查看用量与估算消耗。" : scope === "all" ? "管理所有用户的访问密钥，查看归属和累计消耗。" : "管理自己的访问密钥，查看用量与估算消耗。"} />

      <FormDialog open={Boolean(createdKey)} onOpenChange={open => { if (!open) setCreatedKey("") }} title="查看访问密钥" description={keyLabel} contentClassName="sm:max-w-2xl">
        <Field id="revealed-key" label="完整密钥">
          <Input id="revealed-key" value={createdKey} readOnly spellCheck={false} autoComplete="off" className="font-mono text-sm" onFocus={event => event.target.select()} />
        </Field>
        <div className="mt-4 flex justify-end gap-2">
          <Button type="button" variant="outline" onClick={() => setCreatedKey("")}>关闭</Button>
          <Button type="button" onClick={() => copy(createdKey, "访问密钥")} aria-label="复制访问密钥"><Copy aria-hidden="true" />复制密钥</Button>
        </div>
      </FormDialog>

      <section className="space-y-4" aria-label="访问密钥列表">
      {keys.length > 0 && (
        <div className="flex flex-col gap-3 border-b border-border-default pb-4 xl:flex-row xl:items-center xl:justify-between">
          <p className="text-sm font-medium text-foreground tabular-nums" role="status">{search ? `找到 ${visibleItems.length} / ${keys.length} 个访问密钥` : `共 ${keys.length} 个访问密钥`}</p>
          <div className="flex min-w-0 flex-col gap-3 min-[480px]:flex-row min-[480px]:items-center min-[480px]:justify-end">
            <div className="w-full min-w-0 min-[480px]:w-80">
              <InputGroup>
                <InputGroupInput aria-label="搜索访问密钥" placeholder={!ownOnly && scope === "all" ? "搜索密钥名称或归属用户" : "搜索密钥名称"} type="search" value={query} onChange={event => { setQuery(event.target.value); setPage(1) }} />
                <InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon>
              </InputGroup>
            </div>
            <CossButton onClick={openDialog}>创建访问密钥</CossButton>
          </div>
        </div>
      )}

      {keys.length === 0 ? (
        <EmptyGrid title="还没有访问密钥" description="创建密钥后，客户端才能调用网关。" actionLabel="创建访问密钥" onAction={openDialog} action={<CossButton onClick={openDialog}>创建访问密钥</CossButton>} />
      ) : visibleItems.length === 0 ? (
        <NoSearchResults onClear={() => setQuery("")} />
      ) : (
        <DataTable compact hideSinglePage tableClassName="min-w-[980px] table-fixed" columns={columns} data={visibleItems.slice((currentPage - 1) * 20, currentPage * 20)} page={currentPage} pageSize={20} total={visibleItems.length} onPageChange={setPage} />
      )}

      </section>

      <FormDialog open={dialogOpen} onOpenChange={open => { if (!submitting) setDialogOpen(open) }} title={editing ? "编辑访问密钥" : "创建访问密钥"} description={editing ? `修改 ${editing.owner_email || editing.owner_name || "当前用户"} 的密钥设置。` : "建议按应用或设备命名。"}>
        <AppForm onSubmit={createKey} className="space-y-5" aria-busy={submitting}>
          <Field id="key-name" label="密钥名称"><CossInput id="key-name" name="key-name" value={name} onChange={event => setName(event.target.value)} placeholder="例如：桌面客户端" required /></Field>
          {editing && <Field id="key-status" label="状态"><SelectField id="key-status" value={status} onValueChange={setStatus} disabled={submitting} options={[{ value: "1", label: "已启用" }, { value: "0", label: "已停用" }]} /></Field>}
          <Field id="key-scope" label="模型范围"><SelectField id="key-scope" value={modelScope} onValueChange={setModelScope} disabled={submitting} options={[{value:"all",label:"所有可用模型"},{value:"custom",label:"指定模型别名"}]} /></Field>
          {modelScope === "custom" && <Field id="key-models" label="允许调用的模型" hint="每行一个别名，可使用 chat-* 前缀规则"><Textarea id="key-models" value={modelNames} onChange={event => setModelNames(event.target.value)} disabled={submitting} placeholder={"juno-chat\nimage-edit"} required /></Field>}
          <Field id="key-expiry" label="有效期"><SelectField id="key-expiry" value={expiry} onValueChange={setExpiry} disabled={submitting} options={[...(editing ? [{value:"keep",label:editing.expires_at ? `保留当前：${new Date(editing.expires_at * 1000).toLocaleString("zh-CN")}` : "保留当前：不过期"}] : []),{value:"never",label:"不过期"},{value:"7",label:"从现在起 7 天"},{value:"30",label:"从现在起 30 天"},{value:"90",label:"从现在起 90 天"}]} /></Field>
          <RequestLimitsFields limits={limits} prefix="key-edit" disabled={submitting} />
          <FormError message={formError} />
          <div className="flex justify-end gap-3 pt-1">
            <CossButton type="button" variant="outline" disabled={submitting} onClick={() => setDialogOpen(false)}>取消</CossButton>
            <CossButton type="submit" loading={submitting}>{editing ? "保存修改" : "创建"}</CossButton>
          </div>
        </AppForm>
      </FormDialog>

      <ConfirmDialog open={Boolean(deleteTarget)} onOpenChange={open => { if (!open) setDeleteTarget(null) }} title={`删除“${deleteTarget?.name || ""}”？`} description="删除后无法恢复，使用它的现有请求可能立即失败。" confirmText="确认删除" cancelText="保留" destructive onConfirm={remove} />
    </div>
  )
}
