"use client"

import { useCallback, useEffect, useState } from "react"
import { GuideDocument } from "@/components/guide-document"
import { ModelAccessExamples } from "@/components/model-catalog"
import { SelectField } from "@/components/ui/select-field"
import { BookOpen, Download, Pencil, RotateCcw, Save } from "lucide-react"
import { Button } from "@/components/coss-ui/button"
import { Textarea } from "@/components/coss-ui/textarea"
import { Card } from "@/components/ui/card"
import { ConfirmDialog } from "@/components/confirm-dialog"
import { AccessGuide, type AvailableModel, consoleAPI } from "@/lib/api"
import { useAuthStore } from "@/lib/store"
import { ErrorState, LoadingState, PageHead } from "../_components/console-ui"
import { useConsoleGuard } from "../_components/use-console-guard"
import { toast } from "sonner"

export default function AccessGuidePage() {
  const ready = useConsoleGuard()
  const user = useAuthStore(state => state.user)
  const canEdit = user?.role === "admin"
  const [modelsLoading, setModelsLoading] = useState(true)
  const [models, setModels] = useState<AvailableModel[]>([])
  const [modelError, setModelError] = useState("")
  const [selectedAlias, setSelectedAlias] = useState("")
  const [document, setDocument] = useState<AccessGuide | null>(null)
  const [draft, setDraft] = useState("")
  const [editing, setEditing] = useState(false)
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState("")
  const [saveError, setSaveError] = useState("")
  const [saving, setSaving] = useState(false)
  const [templateOpen, setTemplateOpen] = useState(false)
  const [discardOpen, setDiscardOpen] = useState(false)
  const dirty = Boolean(document && draft !== document.content)
  const draftKey = `junohub-guide-draft:${user?.uid}`
  const load = useCallback(async () => {
    setLoading(true); setLoadError(""); setSaveError("")
    try {
      const data = await consoleAPI.accessGuide()
      setDocument(data); setDraft(data.content)
      if (canEdit) {
        try {
          const stored = sessionStorage.getItem(draftKey)
          if (stored) {
            const saved = JSON.parse(stored) as {content: string; revision: number}
            if (typeof saved.content === "string" && saved.content !== data.content) {
              setDraft(saved.content); setEditing(true)
              setSaveError(saved.revision === data.revision ? "已恢复未保存的草稿。" : "文档已有新版本，已保留你的草稿，请切换阅读模式核对已保存文档后再保存。")
            }
          }
        } catch { /* A blocked browser storage should not prevent reading. */ }
      }
    } catch (error) { setLoadError(error instanceof Error ? error.message : "无法加载接入指引") }
    finally { setLoading(false) }
  }, [canEdit, draftKey])
  useEffect(() => { if (ready) void load() }, [ready, load])
  useEffect(() => {
    if (!document || !canEdit) return
    try {
      if (dirty) sessionStorage.setItem(draftKey, JSON.stringify({content: draft, revision: document.revision}))
      else sessionStorage.removeItem(draftKey)
    } catch { /* Editing still works when browser storage is unavailable. */ }
  }, [draft, document, dirty, canEdit, draftKey])
  useEffect(() => {
    if (!dirty) return
    const warn = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = "" }
    window.addEventListener("beforeunload", warn)
    return () => window.removeEventListener("beforeunload", warn)
  }, [dirty])

  const loadModels = useCallback(async () => {
    setModelsLoading(true); setModelError("")
    try { setModels((await consoleAPI.availableModels()).list) }
    catch (error) { setModelError(error instanceof Error ? error.message : "无法加载模型") }
    finally { setModelsLoading(false) }
  }, [])
  useEffect(() => { if (ready) void loadModels() }, [ready, loadModels])
  const selected = models.find(model => model.alias === selectedAlias)
  const save = async () => {
    if (!document || saving || !canEdit) return
    setSaving(true); setSaveError("")
    try {
      const saved = await consoleAPI.saveAccessGuide({content: draft, revision: document.revision})
      setDocument(saved); setDraft(saved.content); toast.success("接入指引已保存")
    } catch (error) { setSaveError(error instanceof Error ? error.message : "保存失败，草稿已保留。") }
    finally { setSaving(false) }
  }
  const download = () => {
    if (!document) return
    const url = URL.createObjectURL(new Blob([editing ? draft : document.content], {type: "text/markdown;charset=utf-8"}))
    const link = window.document.createElement("a"); link.href = url; link.download = "JunoHub-接入指引.md"; link.click()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
  }
  if (loading) return <LoadingState />
  if (loadError || !document) return <ErrorState message={loadError || "文档加载失败"} onRetry={load} />
  const preview = <GuideDocument content={editing ? draft : document.content} />

  return <div className="space-y-6">
    <PageHead title="接入指引" description="从创建密钥到发起请求，查看 JunoHub 的接入方法。" />
    {!editing && <Card className="min-w-0 gap-4 p-5 sm:p-6"><div className="flex flex-wrap items-center justify-between gap-3"><div><h2 className="text-sm font-semibold">选择模型，查看调用示例</h2><p className="mt-1 text-xs leading-6 text-muted-foreground">使用当前站点地址和模型配置生成示例。</p></div><div className="w-full sm:w-72"><label className="sr-only" htmlFor="guide-model">示例模型</label><SelectField id="guide-model" size="search" value={selectedAlias} onValueChange={setSelectedAlias} placeholder="选择可用模型" options={models.map(model => ({value:model.alias,label:model.alias}))} disabled={modelsLoading || !models.length} /></div></div>{modelError ? <div role="alert" className="text-sm">{modelError}<Button variant="outline" size="sm" className="ms-3" onClick={loadModels}>重试</Button></div> : selected ? <ModelAccessExamples key={selected.alias} model={selected} /> : <p className="text-xs text-muted-foreground">{modelsLoading ? "正在加载可用模型…" : models.length ? "选择一个模型，查看它的接口和三种语言的请求示例。" : "暂无已开放模型，可先阅读下方指引。"}</p>}</Card>}
    <div className="flex flex-wrap items-center justify-between gap-3">
      <p className="text-xs text-muted-foreground" role="status">{dirty ? "有未保存的修改" : document.updated_at ? `更新于 ${new Date(document.updated_at * 1000).toLocaleString("zh-CN")}` : "默认接入指引"}</p>
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="outline" onClick={download}><Download aria-hidden="true" />下载 MD</Button>
        {canEdit && <Button variant="outline" disabled={saving} onClick={() => setEditing(!editing)}>{editing ? <BookOpen aria-hidden="true" /> : <Pencil aria-hidden="true" />}{editing ? "阅读文档" : "编辑文档"}</Button>}
        {canEdit && editing && <>{document.template_content && <Button variant="outline" disabled={saving} onClick={() => setTemplateOpen(true)}>载入接入模板</Button>}<Button variant="ghost" disabled={!dirty || saving} onClick={() => setDiscardOpen(true)}><RotateCcw aria-hidden="true" />撤销修改</Button><Button disabled={!dirty || saving || !draft.trim()} loading={saving} onClick={save}><Save aria-hidden="true" />保存文档</Button></>}
      </div>
    </div>
    {saveError && <div role="alert" className="rounded-lg border border-border-default bg-bg-surface p-3 text-sm leading-6">{saveError}{saveError.includes("其他管理员") && <Button variant="outline" size="sm" className="ms-3" onClick={load}>加载最新版本</Button>}</div>}
    {canEdit && editing ? <div className="grid gap-4 xl:grid-cols-2">
      <Card className="min-w-0 gap-3 p-4 sm:p-5">
        <div><label htmlFor="guide-content" className="text-sm font-semibold">Markdown 源文档</label><p className="mt-1 text-xs leading-5 text-muted-foreground">支持标题、列表、表格、链接和代码块。</p></div>
        <Textarea id="guide-content" name="guide-content" value={draft} onChange={event => { setDraft(event.target.value); setSaveError("") }} disabled={saving} className="w-full [&_textarea]:h-[36rem] [&_textarea]:min-h-[24rem] [&_textarea]:resize-y [&_textarea]:font-mono [&_textarea]:text-sm [&_textarea]:leading-6 [&_textarea]:[field-sizing:fixed]" spellCheck={false} />
      </Card>
      <Card className="min-w-0 gap-4 p-4 sm:p-5"><div className="flex flex-wrap items-center justify-between gap-2"><h2 className="text-sm font-semibold">草稿预览</h2>{saveError.includes("新版本") && <Button variant="ghost" size="sm" onClick={() => setEditing(false)}>查看已保存文档</Button>}</div><div role="region" aria-label="Markdown 草稿预览，可滚动" tabIndex={0} className="max-h-[36rem] overflow-y-auto overscroll-contain rounded-lg focus-visible:outline-2 focus-visible:outline-ring">{preview}</div></Card>
    </div> : <GuideDocument content={document.content} navigation />}
    <ConfirmDialog open={templateOpen} onOpenChange={setTemplateOpen} title="载入新版接入模板？" description="当前编辑草稿将替换为新版模板，已保存文档保持不变；点击保存后才会发布。" confirmText="载入模板" cancelText="保留草稿" onConfirm={() => { setDraft(document.template_content || document.content); setSaveError("") }} />
    <ConfirmDialog open={discardOpen} onOpenChange={setDiscardOpen} title="撤销未保存的修改？" description="文档将恢复到最近保存的内容。" confirmText="撤销修改" cancelText="继续编辑" onConfirm={() => { setDraft(document.content); setSaveError("") }} />
  </div>
}
