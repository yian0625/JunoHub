"use client"

import { useEffect, useState } from "react"
import { Copy, KeyRound, Plus, RotateCw } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ConfirmDialog } from "@/components/confirm-dialog"
import { ConsoleKey, consoleAPI } from "@/lib/api"
import { toast } from "sonner"
import { AddCard, EmptyGrid, ErrorState, Field, FormDialog, FormError, LoadingState, PageHead, ResourceCard, ResourceGrid } from "../_components/console-ui"
import { useConsoleGuard } from "../_components/use-console-guard"

export default function KeysPage() {
  const isUser = useConsoleGuard()
  const [keys, setKeys] = useState<ConsoleKey[]>([])
  const [createdKey, setCreatedKey] = useState("")
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState("")
  const [formError, setFormError] = useState("")
  const [submitting, setSubmitting] = useState(false)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<{ id: number; name: string } | null>(null)
  const [name, setName] = useState("默认密钥")

  const load = async () => {
    setLoading(true); setLoadError("")
    try { const data = await consoleAPI.keyList(); setKeys(data.list || []) }
    catch (error) { const message = error instanceof Error ? error.message : "无法加载访问密钥，请检查网络后重试。"; setLoadError(message); toast.error(message) }
    finally { setLoading(false) }
  }

  useEffect(() => { if (isUser) load() }, [isUser])

  const openDialog = () => { setName("默认密钥"); setFormError(""); setDialogOpen(true) }

  const createKey = async (event: React.FormEvent) => {
    event.preventDefault(); setFormError(""); setSubmitting(true)
    try { const key = await consoleAPI.keyCreate({ name: name.trim() }); setCreatedKey(key.key || ""); setDialogOpen(false); await load(); toast.success("访问密钥已创建") }
    catch (error) { setFormError(error instanceof Error ? error.message : "无法创建密钥，请重试。") }
    finally { setSubmitting(false) }
  }

  const copy = async (text: string, label: string) => { await navigator.clipboard.writeText(text); toast.success(`${label}已复制`) }

  const remove = async () => {
    if (!deleteTarget) return
    try { await consoleAPI.keyDelete(deleteTarget.id); setDeleteTarget(null); await load(); toast.success("已删除") }
    catch (error) { toast.error(error instanceof Error ? error.message : "无法删除，请稍后重试。") }
  }

  if (loading && !keys.length) return <LoadingState />
  if (loadError && !keys.length) return <ErrorState message={loadError} onRetry={load} />

  return (
    <div className="view-enter space-y-8">
      <PageHead title="访问密钥" description="为不同应用签发独立凭证，方便单独撤销。" action={<Button size="lg" onClick={openDialog}><Plus aria-hidden="true" />创建访问密钥</Button>} />

      {createdKey && (
        <div className="surface-card rounded-2xl border-warning-solid/40 p-5" style={{ background: "var(--color-warning-subtle)" }}>
          <div className="flex gap-3">
            <KeyRound className="mt-0.5 size-5 shrink-0 text-warning-text" aria-hidden="true" />
            <div className="min-w-0 flex-1">
              <p className="text-sm font-semibold text-warning-text">立即保存新密钥</p>
              <p className="mt-1 text-sm leading-6 text-warning-text">出于安全考虑，完整密钥只显示这一次。</p>
              <div className="mt-4 flex items-center gap-2 rounded-xl bg-bg-surface p-2 ps-3">
                <code className="min-w-0 flex-1 break-all text-sm"><bdi>{createdKey}</bdi></code>
                <Button variant="outline" size="icon" onClick={() => copy(createdKey, "访问密钥")} aria-label="复制新访问密钥"><Copy aria-hidden="true" /></Button>
              </div>
            </div>
          </div>
        </div>
      )}

      {keys.length === 0 ? (
        <EmptyGrid title="还没有访问密钥" description="创建密钥后，客户端才能调用网关。" actionLabel="创建访问密钥" onAction={openDialog} />
      ) : (
        <ResourceGrid>
          {keys.map(item => (
            <ResourceCard
              key={item.id}
              badge={<KeyRound className="size-[18px]" strokeWidth={1.75} aria-hidden="true" />}
              title={item.name}
              meta={`${item.key_masked} · ${item.total_requests || 0} 次请求`}
              status={{ active: item.status === 1, label: item.status === 1 ? "已启用" : "已停用" }}
              onDelete={() => setDeleteTarget({ id: item.id, name: item.name })}
              deleteLabel={`删除 ${item.name}`}
            />
          ))}
          <AddCard label="创建访问密钥" onClick={openDialog} />
        </ResourceGrid>
      )}

      <FormDialog open={dialogOpen} onOpenChange={setDialogOpen} title="创建访问密钥" description="建议按应用或设备命名。">
        <form onSubmit={createKey} className="space-y-5" aria-busy={submitting}>
          <Field id="key-name" label="密钥名称"><Input id="key-name" name="key-name" value={name} onChange={event => setName(event.target.value)} placeholder="例如：桌面客户端" required /></Field>
          <FormError message={formError} />
          <div className="flex gap-3 pt-1">
            <Button type="button" variant="outline" className="flex-1" onClick={() => setDialogOpen(false)}>取消</Button>
            <Button type="submit" className="flex-1" disabled={submitting}>{submitting && <RotateCw className="animate-spin" aria-hidden="true" />}创建</Button>
          </div>
        </form>
      </FormDialog>

      <ConfirmDialog open={Boolean(deleteTarget)} onOpenChange={open => { if (!open) setDeleteTarget(null) }} title={`删除“${deleteTarget?.name || ""}”？`} description="删除后无法恢复，使用它的现有请求可能立即失败。" confirmText="确认删除" cancelText="保留" destructive onConfirm={remove} />
    </div>
  )
}
