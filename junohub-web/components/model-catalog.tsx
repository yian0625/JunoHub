"use client"

import Link from "next/link"
import { useId, useState, useSyncExternalStore, type ReactNode } from "react"
import { ArrowUpRight, BookOpen, Copy, SearchIcon, Trash2 } from "lucide-react"
import { Button } from "@/components/coss-ui/button"
import { Badge } from "@/components/coss-ui/badge"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/coss-ui/input-group"
import { AppTooltip } from "@/components/ui/tooltip"
import { SelectField } from "@/components/ui/select-field"
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs"
import { ProviderAvatar } from "@/components/provider-avatar"
import { ModelBillingSummary } from "@/components/model-billing-summary"
import { FormDialog, ResourceCard, ResourceGrid } from "@/app/(admin)/console/_components/console-ui"
import type { AvailableModel, ConsoleProvider } from "@/lib/api"
import { modelInterface } from "@/lib/model-api"
import { modelEndpoint, modelExample, modelUploadExample, modelPayload, modelType, modelTypeLabels, type ExampleLanguage } from "@/lib/model-access"
import { toast } from "sonner"

export interface CatalogModel extends AvailableModel { enabled?: number }
export async function copyModelText(text: string) {
  try { await navigator.clipboard.writeText(text); toast.success("已复制") }
  catch { toast.error("复制失败，请手动选择并复制。") }
}

export function ModelCatalogSearch({ query, onQueryChange, type, onTypeChange, label, count, total, action }: {
  query: string; onQueryChange: (value: string) => void; type: string; onTypeChange: (value: string) => void; label: string; count: number; total: number; action?: ReactNode
}) {
  return <div className="flex flex-col gap-4 border-b border-border-default pb-4">
    <div className="flex flex-wrap items-center gap-3">
      <div className="w-full min-w-0 sm:w-80"><InputGroup><InputGroupInput aria-label={`搜索${label}`} placeholder={`搜索${label}`} type="search" value={query} onChange={event => onQueryChange(event.target.value)} /><InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon></InputGroup></div>
      <div className="w-full sm:w-40"><label htmlFor="catalog-type" className="sr-only">模型类型</label><SelectField id="catalog-type" size="search" value={type} onValueChange={onTypeChange} options={[{value:"all",label:"全部模型类型"}, ...Object.entries(modelTypeLabels).map(([value,label]) => ({value,label}))]} /></div>
      {action && <div className="sm:ms-auto">{action}</div>}
    </div>
    <p className="text-xs text-muted-foreground tabular-nums" role="status">{query.trim() || type !== "all" ? `找到 ${count} / ${total} 个${label}` : `共 ${total} 个${label}`}</p>
  </div>
}

export function ModelCatalogCard({ model, provider, providerName, actions, onDelete, onDetails }: {
  model: CatalogModel; provider?: Pick<ConsoleProvider, "provider_type" | "platform" | "avatar_url">; providerName?: string; actions?: ReactNode; onDelete?: () => void; onDetails: () => void
}) {
  const category = modelType(model.api_type)
  const type = modelTypeLabels[category]
  const capabilities = model.capabilities.filter(value => value !== category)
  return <ResourceCard compact headingLevel={3} title={model.alias} badge={<ProviderAvatar provider={provider || model.icon || {provider_type:""}} />} status={{active:model.enabled !== 0,label:model.enabled === 0 ? "已停用" : providerName ? "已启用" : "已开放"}} actions={<div className="flex w-full flex-wrap items-center gap-2">{onDelete && <AppTooltip content={`删除 ${model.alias}`}><Button size="icon-sm" variant="ghost" aria-label={`删除 ${model.alias}`} onClick={onDelete} className="me-auto text-muted-foreground hover:text-danger-text"><Trash2 aria-hidden="true" /></Button></AppTooltip>}<Button size="sm" variant="outline" onClick={onDetails}>价格与接入<ArrowUpRight aria-hidden="true" /></Button>{actions}</div>}>
    <div className="flex flex-wrap items-center gap-2"><Badge variant="outline">{type}</Badge>{providerName && <span className="text-xs text-muted-foreground [overflow-wrap:anywhere]">{providerName}</span>}</div>
    <dl className="my-4 space-y-2 text-xs leading-5">
      {capabilities.length > 0 && <div className="grid grid-cols-[3rem_minmax(0,1fr)] gap-3"><dt className="text-muted-foreground">能力</dt><dd className="[overflow-wrap:anywhere]">{capabilities.map(value => modelTypeLabels[value] || value).join(" · ")}</dd></div>}
      <div className="grid grid-cols-[3rem_minmax(0,1fr)] gap-3"><dt className="text-muted-foreground">接口</dt><dd className="[overflow-wrap:anywhere]">{modelInterface(model.api_type).label}</dd></div>
      <div className="grid grid-cols-[3rem_minmax(0,1fr)] gap-3"><dt className="text-muted-foreground">返回</dt><dd>{model.response_mode === "task" ? "异步任务 · 查询结果" : "直接响应"}</dd></div>
    </dl>
    <ModelBillingSummary config={model.billing_config} input={model.input_price_per_million} output={model.output_price_per_million} inputOnly={model.api_type === "embeddings"} />
    <div className="mt-4 flex min-w-0 items-center gap-2 rounded-lg bg-bg-sunken/60 px-3 py-2"><div className="min-w-0 flex-1"><p className="mb-1 text-[11px] text-muted-foreground">调用模型名</p><code className="block break-all text-xs leading-5">{model.alias}</code></div><Button size="icon-sm" variant="ghost" aria-label={`复制模型名 ${model.alias}`} onClick={() => copyModelText(model.alias)}><Copy aria-hidden="true" /></Button></div>
  </ResourceCard>
}

const subscribeToOrigin = () => () => {}
const getOrigin = () => window.location.origin
const getServerOrigin = () => ""

export function ModelAccessExamples({ model }: { model: CatalogModel }) {
  const origin = useSyncExternalStore(subscribeToOrigin, getOrigin, getServerOrigin)
  const endpoint = modelEndpoint(model.api_type, model.endpoint)
  const canUpload = model.request_formats?.includes("multipart/form-data") === true
  const [requestFormat, setRequestFormat] = useState(canUpload ? "multipart" : "json")
  const formatId = useId()
  const upload = canUpload && requestFormat === "multipart"
  const languages: {value:ExampleLanguage;label:string}[] = [{value:"curl",label:"curl"},{value:"python",label:"Python"},{value:"javascript",label:"JavaScript"}]
  return <div className="min-w-0 space-y-5">
    <div className="space-y-3">
      <div className="flex min-w-0 items-center gap-3 rounded-xl border border-border-default bg-bg-sunken/50 p-3"><Badge variant="outline">POST</Badge><code className="min-w-0 flex-1 break-all text-xs leading-5">{origin}{endpoint}</code><Button size="icon-sm" variant="ghost" aria-label="复制客户端调用地址" disabled={!origin} onClick={() => copyModelText(origin + endpoint)}><Copy aria-hidden="true" /></Button></div>
      <p className="text-xs leading-6 text-muted-foreground">在「我的密钥」创建访问密钥，并将其设置为环境变量 <code>JUNOHUB_API_KEY</code>。请求的 <code>model</code> 使用 <code className="break-all">{model.alias}</code>。</p>
      {canUpload && <div className="space-y-2"><label htmlFor={formatId} className="text-xs font-medium">请求类型</label><SelectField id={formatId} value={requestFormat} onValueChange={setRequestFormat} options={[{value:"multipart",label:"文件表单 · multipart/form-data"},{value:"json",label:"JSON · application/json"}]} /><p className="text-xs leading-6 text-muted-foreground">文件表单总大小最多 32MiB。文件字段名及其他参数沿用上游要求；网关只替换 model，不转换 JSON 与文件表单格式。</p></div>}
      {!upload && !modelPayload(model.alias, model.api_type) && <p className="text-xs leading-6 text-muted-foreground">此接口转发供应商的 JSON 请求字段。以下示例读取你准备的 request.json；具体字段由管理员根据该模型的供应商文档提供。具体字段须与上游接口支持的请求格式一致。</p>}
    </div>
    <Tabs defaultValue="curl" className="min-w-0 gap-3"><TabsList aria-label="示例语言">{languages.map(item => <TabsTrigger key={item.value} value={item.value}>{item.label}</TabsTrigger>)}</TabsList>
      {languages.map(item => { const code = upload ? modelUploadExample(model.alias, model.api_type, endpoint, item.value, origin) : modelExample(model.alias, model.api_type, endpoint, item.value, origin); return <TabsContent key={item.value} value={item.value} className="min-w-0 space-y-2"><div className="flex items-center justify-between gap-2"><span className="text-xs text-muted-foreground">{item.label === "JavaScript" ? upload ? "Node.js 20+" : "Node.js 18+" : item.label === "Python" ? upload ? "Python 3 · pip install requests" : "Python 3 · 标准库" : "命令行"}</span><Button size="sm" variant="outline" onClick={() => copyModelText(code)}><Copy aria-hidden="true" />复制示例</Button></div><pre tabIndex={0} aria-label={`${item.label} 请求示例，可横向滚动`} className="max-w-full overflow-x-auto rounded-xl border border-border-default bg-bg-sunken p-4 text-xs leading-6 focus-visible:outline-2 focus-visible:outline-ring"><code>{code}</code></pre></TabsContent> })}
    </Tabs>
    {model.response_mode === "task" && <div className="space-y-2 rounded-xl border border-border-default p-4 text-xs leading-6"><h3 className="font-semibold">异步任务</h3><p>HTTP 202 表示已提交任务。保存响应中的 task_id 与 query_url，使用提交时的同一个访问密钥，以 5–10 秒间隔 GET 查询结果。succeeded 为成功，failed 为失败。</p><p className="text-muted-foreground">后台持续查询并结算，关闭页面不会停止任务。提交失败或超时后先查看请求日志，避免重复生成。</p></div>}
  </div>
}

export function ModelCatalogDetails({ model, open, onOpenChange, upstream }: { model: CatalogModel | null; open: boolean; onOpenChange: (open: boolean) => void; upstream?: ReactNode }) {
  return <FormDialog open={open} onOpenChange={onOpenChange} title={model?.alias || "模型详情"} description="查看模型价格、客户端接口与调用示例。" contentClassName="sm:max-w-3xl" footer={<div className="flex flex-wrap items-center justify-between gap-3"><Button variant="outline" render={<Link href="/console/guide" />}><BookOpen aria-hidden="true" />完整接入指引</Button><Button variant="outline" onClick={() => onOpenChange(false)}>关闭</Button></div>}>
    {model && <div className="space-y-6"><section aria-label="模型价格" className="space-y-2"><h3 className="text-sm font-semibold">计费价格</h3><ModelBillingSummary config={model.billing_config} input={model.input_price_per_million} output={model.output_price_per_million} inputOnly={model.api_type === "embeddings"} /><p className="text-xs leading-6 text-muted-foreground">按实际用量和调用时的价格配置结算。未配置价格或缺少实际用量时标记未计价；价格为 0 才表示免费。</p></section><section aria-label="API 接入" className="space-y-3"><h3 className="text-sm font-semibold">API 接入</h3><ModelAccessExamples key={model.alias} model={model} /></section>{upstream && <section className="space-y-3 border-t border-border-default pt-5"><h3 className="text-sm font-semibold">上游配置</h3>{upstream}</section>}</div>}
  </FormDialog>
}

export function ModelCatalogGrid<T>({ items, typeOf, children }: {items:T[]; typeOf:(item:T) => string; children:(item:T) => ReactNode}) {
  const groups = Object.entries(modelTypeLabels).map(([type,label]) => ({type,label,items:items.filter(item => typeOf(item) === type)})).filter(group => group.items.length)
  return <div className="space-y-7">{groups.map(group => <section key={group.type} aria-label={`${group.label}模型`} className="space-y-3"><div className="flex items-center gap-2"><h2 className="text-sm font-semibold">{group.label}</h2><span className="rounded-md bg-bg-sunken px-2 py-0.5 text-xs text-muted-foreground tabular-nums">{group.items.length}</span></div><ResourceGrid>{group.items.map(children)}</ResourceGrid></section>)}</div>
}
