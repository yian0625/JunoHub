"use client"

import { useEffect, useState } from "react"
import { Network, Plus, RotateCw } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ConfirmDialog } from "@/components/confirm-dialog"
import { ConsoleModel, ConsoleProvider, consoleAPI } from "@/lib/api"
import { toast } from "sonner"
import { AddCard, EmptyGrid, ErrorState, Field, FormDialog, FormError, LoadingState, PageHead, ResourceCard, ResourceGrid } from "../_components/console-ui"
import { useConsoleGuard } from "../_components/use-console-guard"

export default function ModelsPage() {
  const isUser = useConsoleGuard()
  const [providers, setProviders] = useState<ConsoleProvider[]>([])
  const [models, setModels] = useState<ConsoleModel[]>([])
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState("")
  const [formError, setFormError] = useState("")
  const [submitting, setSubmitting] = useState(false)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<{ id: number; name: string } | null>(null)
  const [form, setForm] = useState({ provider_id: "", alias: "", upstream_model: "" })

  const load = async () => {
    setLoading(true); setLoadError("")
    try {
      const [providerData, modelData] = await Promise.all([consoleAPI.providerList(), consoleAPI.modelList()])
      setProviders(providerData.list || []); setModels(modelData.list || [])
    } catch (error) { const message = error instanceof Error ? error.message : "无法加载模型路由，请检查网络后重试。"; setLoadError(message); toast.error(message) }
    finally { setLoading(false) }
  }

  useEffect(() => { if (isUser) load() }, [isUser])

  const openDialog = () => {
    if (!providers.length) { toast.error("请先添加供应商，再创建模型路由。"); return }
    setForm({ provider_id: String(providers[0]?.id || ""), alias: "", upstream_model: "" }); setFormError(""); setDialogOpen(true)
  }

  const addModel = async (event: React.FormEvent) => {
    event.preventDefault(); setFormError(""); setSubmitting(true)
    try {
      await consoleAPI.modelCreate({ provider_id: Number(form.provider_id), alias: form.alias.trim(), upstream_model: form.upstream_model.trim(), capabilities: ["chat"] })
      setDialogOpen(false); await load(); toast.success("模型路由已添加")
    } catch (error) { setFormError(error instanceof Error ? error.message : "无法添加模型路由，请检查填写内容后重试。") }
    finally { setSubmitting(false) }
  }

  const remove = async () => {
    if (!deleteTarget) return
    try { await consoleAPI.modelDelete(deleteTarget.id); setDeleteTarget(null); await load(); toast.success("已删除") }
    catch (error) { toast.error(error instanceof Error ? error.message : "无法删除，请稍后重试。") }
  }

  if (loading && !models.length) return <LoadingState />
  if (loadError && !models.length) return <ErrorState message={loadError} onRetry={load} />

  return (
    <div className="view-enter space-y-8">
      <PageHead title="模型路由" description="为上游模型设置稳定、易记的对外名称，客户端只需要使用别名。" action={<Button size="lg" onClick={openDialog}><Plus aria-hidden="true" />添加模型路由</Button>} />

      {models.length === 0 ? (
        <EmptyGrid title="还没有模型路由" description={providers.length ? "把上游模型映射为对外别名，客户端即可调用。" : "请先添加供应商，再把上游模型映射为对外别名。"} actionLabel="添加模型路由" onAction={openDialog} />
      ) : (
        <ResourceGrid>
          {models.map(item => (
            <ResourceCard
              key={item.id}
              badge={<Network className="size-[18px]" strokeWidth={1.75} aria-hidden="true" />}
              title={item.alias}
              meta={`${item.provider_name || item.provider_type} → ${item.upstream_model}`}
              status={{ active: item.enabled === 1, label: item.enabled === 1 ? "已启用" : "已停用" }}
              onDelete={() => setDeleteTarget({ id: item.id, name: item.alias })}
              deleteLabel={`删除 ${item.alias}`}
            />
          ))}
          <AddCard label="添加模型路由" onClick={openDialog} />
        </ResourceGrid>
      )}

      <FormDialog open={dialogOpen} onOpenChange={setDialogOpen} title="添加模型路由" description="客户端只需要使用你设置的别名。">
        <form onSubmit={addModel} className="space-y-5" aria-busy={submitting}>
          <Field id="model-provider" label="供应商"><select id="model-provider" name="model-provider" className="field-control w-full px-3.5 text-base sm:text-sm" value={form.provider_id} onChange={event => setForm({ ...form, provider_id: event.target.value })} required><option value="">选择供应商</option>{providers.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select></Field>
          <Field id="model-alias" label="对外别名"><Input id="model-alias" name="model-alias" spellCheck={false} value={form.alias} onChange={event => setForm({ ...form, alias: event.target.value })} placeholder="例如：fast-chat" required /></Field>
          <Field id="upstream-model" label="上游模型名称"><Input id="upstream-model" name="upstream-model" spellCheck={false} value={form.upstream_model} onChange={event => setForm({ ...form, upstream_model: event.target.value })} placeholder="例如：gpt-4o" required /></Field>
          <FormError message={formError} />
          <div className="flex gap-3 pt-1">
            <Button type="button" variant="outline" className="flex-1" onClick={() => setDialogOpen(false)}>取消</Button>
            <Button type="submit" className="flex-1" disabled={submitting}>{submitting && <RotateCw className="animate-spin" aria-hidden="true" />}添加</Button>
          </div>
        </form>
      </FormDialog>

      <ConfirmDialog open={Boolean(deleteTarget)} onOpenChange={open => { if (!open) setDeleteTarget(null) }} title={`删除“${deleteTarget?.name || ""}”？`} description="删除后无法恢复，使用它的客户端将无法再调用该模型。" confirmText="确认删除" cancelText="保留" destructive onConfirm={remove} />
    </div>
  )
}
