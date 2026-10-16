"use client"

import { ModelApiEditor } from "@/components/model-api-editor"
import { ModelCatalogCard, ModelCatalogDetails, ModelCatalogSearch, ModelCatalogGrid, type CatalogModel } from "@/components/model-catalog"
import { modelType, modelEndpoint, modelTypeLabels } from "@/lib/model-access"
import { billingUnits, billingUnitLabels, requiresQuantityField, type BillingUnit } from "@/lib/model-billing"
import { ModelRouteOptions } from "@/components/model-route-options"
import { defaultModelApi, newModelApi, modelInterface, upstreamInterface, routeConfigError, upstreamAddress } from "@/lib/model-api"

import { RequestLimitsFields, useRequestLimits } from "@/components/resource-limits-fields"
import { UpstreamModelPicker } from "@/components/upstream-model-picker"
import { AppForm } from "@/components/ui/app-form"

import { useEffect, useState } from "react"
import Link from "next/link"
import { BookOpen, Pencil, RotateCw } from "lucide-react"
import { Button } from "@/components/coss-ui/button"
import { SelectField } from "@/components/ui/select-field"
import { Input } from "@/components/ui/input"
import { ConfirmDialog } from "@/components/confirm-dialog"
import { ConsoleModel, ConsoleProvider, consoleAPI } from "@/lib/api"
import { toast } from "sonner"
import { EmptyGrid, ErrorState, Field, FormDialog, FormError, LoadingState, PageHead, NoSearchResults } from "../_components/console-ui"
import { useConsoleGuard } from "../_components/use-console-guard"

export default function ModelsPage() {
  const isUser = useConsoleGuard()
  const [query, setQuery] = useState("")
  const [type, setType] = useState("all")
  const [detail, setDetail] = useState<ConsoleModel | null>(null)
  const catalogModel = (item: ConsoleModel): CatalogModel => ({ ...item, input_price_per_million: item.input_price_per_million ?? null, output_price_per_million: item.output_price_per_million ?? null, api_type: item.api_config?.api_type || "auto", endpoint: modelEndpoint(item.api_config?.api_type), response_mode: item.api_config?.response_mode || "direct", request_formats: ["application/json", ...(providers.find(provider => provider.id === item.provider_id)?.provider_type !== "sub2api" && ["images_edits", "audio_transcriptions", "audio_translations"].includes(item.api_config?.api_type || "") ? ["multipart/form-data"] : [])] })
  const [providers, setProviders] = useState<ConsoleProvider[]>([])
  const [models, setModels] = useState<ConsoleModel[]>([])
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState("")
  const [formError, setFormError] = useState("")
  const [submitting, setSubmitting] = useState(false)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<{ id: number; name: string } | null>(null)
  const [editing, setEditing] = useState<ConsoleModel | null>(null)
  const [pricesOpen, setPricesOpen] = useState(false)
  const [form, setForm] = useState({ provider_id: "", alias: "", upstream_model: "", request_path: "", api_config: { ...defaultModelApi }, input_price: "", output_price: "", billing_unit: "token" as BillingUnit, unit_price: "", quantity_pointer: "" })
  const limits = useRequestLimits("model", editing?.id ?? null, dialogOpen)
  const inputOnly = form.api_config.api_type === "embeddings"

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
    setEditing(null); setPricesOpen(false); setForm({ provider_id: String(providers[0]?.id || ""), alias: "", upstream_model: "", request_path: "", api_config: { ...(providers[0]?.provider_type === "sub2api" ? defaultModelApi : newModelApi) }, input_price: "", output_price: "", billing_unit: "token" as BillingUnit, unit_price: "", quantity_pointer: "" }); setFormError(""); setDialogOpen(true)
  }

  const editModel = (item: ConsoleModel) => {
    setEditing(item)
    const config = { ...defaultModelApi, ...item.api_config }
    const managed = providers.find(provider => provider.id === item.provider_id)?.provider_type === "sub2api"
    if (!managed && !config.upstream_api_type && config.api_type !== "auto") config.upstream_api_type = config.api_type
    setForm({ provider_id: String(item.provider_id), alias: item.alias, upstream_model: item.upstream_model, request_path: item.request_path || "", api_config: config, input_price: item.input_price_per_million == null ? "" : String(item.input_price_per_million), output_price: item.output_price_per_million == null ? "" : String(item.output_price_per_million), billing_unit: item.billing_config?.unit || "token", unit_price: item.billing_config?.price_per_unit == null ? "" : String(item.billing_config.price_per_unit), quantity_pointer: item.billing_config?.quantity_pointer || "" })
    setPricesOpen(item.input_price_per_million != null || item.output_price_per_million != null || (item.billing_config?.unit && item.billing_config.unit !== "token") || item.billing_config?.price_per_unit != null)
    setFormError(""); setDialogOpen(true)
  }
  const providerName = (item: ConsoleModel) => providers.find(provider => provider.id === item.provider_id)?.name || item.provider_name || "供应商不可用"

  const addModel = async (event: React.FormEvent) => {
    event.preventDefault(); if (submitting) return; setFormError("");
    if (!form.upstream_model.trim()) { setFormError("请选择或填写上游模型。"); document.getElementById("upstream-model")?.focus(); return }
    if (!form.alias.trim()) { setFormError("请输入对外别名。"); return }
    const managed = providers.find(provider => provider.id === Number(form.provider_id))?.provider_type === "sub2api"
    const configError = routeConfigError(form.api_config, form.request_path, managed)
    if (configError) { document.getElementById(configError.field)?.focus(); return }
    const inputPrice = form.input_price.trim() === "" ? null : Number(form.input_price)
    const outputPrice = inputOnly || form.output_price.trim() === "" ? null : Number(form.output_price)
    const unit = inputOnly ? "token" : form.billing_unit
    const unitPrice = form.unit_price.trim() === "" ? null : Number(form.unit_price)
    if ((requiresQuantityField(unit) || (unit === "image" && form.quantity_pointer.trim())) && !form.quantity_pointer.trim().startsWith("/")) { setPricesOpen(true); setFormError("请填写供应商响应的实际用量字段，例如 /usage/duration_seconds。"); document.getElementById("model-quantity-pointer")?.focus(); return }
    if ([inputPrice, outputPrice, ...(unit === "token" ? [] : [unitPrice])].some(value => value !== null && (!Number.isFinite(value) || value < 0 || value > 1_000_000_000))) { setPricesOpen(true); setFormError("价格须为 0–1000000000 之间的数字，留空表示未配置。"); return }
    setSubmitting(true)
    try {
      const payload = { request_limits: limits.value(), billing_config: { unit, price_per_unit: unit === "token" ? null : unitPrice, quantity_pointer: unit === "token" || unit === "request" ? "" : form.quantity_pointer.trim() }, provider_id: Number(form.provider_id), alias: form.alias.trim(), upstream_model: form.upstream_model.trim(), request_path: form.request_path.trim(), api_config: form.api_config, capabilities: editing && modelType(editing.api_config?.api_type) === modelType(form.api_config.api_type) ? editing.capabilities : [modelInterface(form.api_config.api_type).capability], input_price_per_million: inputPrice, output_price_per_million: outputPrice }
      if (editing) await consoleAPI.modelUpdate({ ...payload, id: editing.id })
      else await consoleAPI.modelCreate(payload)
      setDialogOpen(false); await load(); toast.success(editing ? "模型路由已保存" : "模型路由已添加")
    } catch (error) { setFormError(error instanceof Error ? error.message : "无法添加模型路由，请检查填写内容后重试。") }
    finally { setSubmitting(false) }
  }

  const remove = async () => {
    if (!deleteTarget) return
    try { await consoleAPI.modelDelete(deleteTarget.id); setDeleteTarget(null); await load(); toast.success("已删除") }
    catch (error) { toast.error(error instanceof Error ? error.message : "无法删除，请稍后重试。") }
  }

  const search = query.trim().toLocaleLowerCase()
  const visibleItems = models.filter(item => (type === "all" || modelType(item.api_config?.api_type) === type) && [modelTypeLabels[modelType(item.api_config?.api_type)], item.alias, providerName(item), item.upstream_model, modelInterface(item.api_config?.api_type).label, item.request_path].join(" ").toLocaleLowerCase().includes(search))

  if (loading && !models.length) return <LoadingState />
  if (loadError && !models.length) return <ErrorState message={loadError} onRetry={load} />

  return (
    <div className="view-enter space-y-8">
      <PageHead title="模型路由" description="为上游模型设置稳定、易记的对外名称，客户端只需要使用别名。" action={<Button variant="outline" render={<Link href="/console/guide" />}><BookOpen aria-hidden="true" />查看接入指引</Button>} />

      <section className="space-y-4" aria-label="模型路由列表">
      {models.length > 0 && <ModelCatalogSearch query={query} onQueryChange={setQuery} type={type} onTypeChange={setType} label="模型路由" count={visibleItems.length} total={models.length} action={<Button onClick={openDialog}>添加模型路由</Button>} />}

      {models.length === 0 ? (
        <EmptyGrid title="还没有模型路由" description={providers.length ? "把上游模型映射为对外别名，客户端即可调用。" : "请先添加供应商，再把上游模型映射为对外别名。"} actionLabel="添加模型路由" onAction={openDialog} action={<Button onClick={openDialog}>添加模型路由</Button>} />
      ) : visibleItems.length === 0 ? (
        <NoSearchResults onClear={() => { setQuery(""); setType("all") }} />
      ) : (
        <ModelCatalogGrid items={visibleItems} typeOf={item => modelType(item.api_config?.api_type)}>
          {item => <ModelCatalogCard key={item.id} model={catalogModel(item)} provider={providers.find(provider => provider.id === item.provider_id) || {provider_type:item.provider_type || ""}} providerName={providerName(item)} onDetails={() => setDetail(item)} onDelete={() => setDeleteTarget({id:item.id,name:item.alias})} actions={<Button size="sm" variant="outline" onClick={() => editModel(item)}><Pencil aria-hidden="true" />编辑</Button>} />}
        </ModelCatalogGrid>
      )}

      </section>

      <ModelCatalogDetails model={detail ? catalogModel(detail) : null} open={Boolean(detail)} onOpenChange={open => { if (!open) setDetail(null) }} upstream={detail && <dl className="space-y-2 text-xs leading-6"><div><dt className="text-muted-foreground">供应商</dt><dd>{providerName(detail)}</dd></div><div><dt className="text-muted-foreground">上游模型</dt><dd className="break-all font-mono">{detail.upstream_model}</dd></div><div><dt className="text-muted-foreground">上游请求格式</dt><dd>{upstreamInterface(detail.api_config).label}</dd></div><div><dt className="text-muted-foreground">完整上游地址</dt><dd className="break-all font-mono">{upstreamAddress(providers.find(provider => provider.id === detail.provider_id)?.base_url || "", detail.request_path || "", detail.api_config) || "由账户引擎选择"}</dd></div></dl>} />
      <FormDialog open={dialogOpen} onOpenChange={open => { if (!submitting) setDialogOpen(open) }} title={editing ? "编辑模型路由" : "添加模型路由"} description="连接上游模型，设置客户端使用的别名和接口。" contentClassName="sm:max-w-4xl" footer={<div className="space-y-3">
        <FormError message={formError} />
        <div className="flex justify-end gap-3">
          <Button type="button" variant="outline" className="min-w-20" disabled={submitting} onClick={() => setDialogOpen(false)}>取消</Button>
          <Button type="submit" form="model-route-form" className="min-w-24" disabled={submitting}>{submitting && <RotateCw className="animate-spin" aria-hidden="true" />}{editing ? "保存" : "添加路由"}</Button>
        </div>
      </div>}>
        <AppForm id="model-route-form" onSubmit={addModel} aria-busy={submitting}>
          <fieldset disabled={submitting} className="min-w-0 space-y-5">
          <ModelApiEditor key={`${dialogOpen}:${editing?.id || "new"}:${form.provider_id}`} providerId={Number(form.provider_id)} providerBaseUrl={providers.find(item => item.id === Number(form.provider_id))?.base_url || ""} upstreamModel={form.upstream_model} config={form.api_config} requestPath={form.request_path} managed={providers.find(item => item.id === Number(form.provider_id))?.provider_type === "sub2api"} onChange={(config, path) => { setFormError(""); setForm(previous => ({ ...previous, api_config: config, request_path: path })) }}
            upstreamFields={<>
              <Field id="model-provider" label="供应商"><SelectField id="model-provider" name="model-provider" value={form.provider_id} onValueChange={value => setForm(previous => {
                const wasManaged = providers.find(item => item.id === Number(previous.provider_id))?.provider_type === "sub2api"
                const managed = providers.find(item => item.id === Number(value))?.provider_type === "sub2api"
                return { ...previous, provider_id: value, upstream_model: "", ...(wasManaged !== managed ? { request_path: "", api_config: { ...(managed ? defaultModelApi : newModelApi) } } : {}) }
              })} options={providers.map(item => ({ value: String(item.id), label: item.name }))} placeholder="选择供应商" required /></Field>
              <UpstreamModelPicker compact id="upstream-model" key={`${dialogOpen}:${editing?.id || "new"}:${form.provider_id}`} providerId={Number(form.provider_id)} value={form.upstream_model} onChange={value => setForm(previous => ({ ...previous, upstream_model: value, alias: !previous.alias || previous.alias === previous.upstream_model ? value : previous.alias }))} />
            </>}
            gatewayFields={<Field id="model-alias" label="对外别名"><Input id="model-alias" name="model-alias" spellCheck={false} value={form.alias} onChange={event => setForm({ ...form, alias: event.target.value })} placeholder="例如：fast-chat" required /></Field>}
            gatewayOptions={<><RequestLimitsFields limits={limits} prefix="model-route" disabled={submitting} /><ModelRouteOptions label="模型价格" summary={`可选 · 美元／${billingUnitLabels[inputOnly ? "token" : form.billing_unit]}`} open={pricesOpen} onOpenChange={setPricesOpen}>
            {!inputOnly && <Field id="model-billing-unit" label="计费方式"><SelectField id="model-billing-unit" value={form.billing_unit} onValueChange={value => setForm(previous => ({ ...previous, billing_unit: value as BillingUnit }))} options={billingUnits} /></Field>}
            {(inputOnly || form.billing_unit === "token") ? <div className={`grid gap-4 ${inputOnly ? "" : "sm:grid-cols-2"}`}>
              <Field id="model-input-price" label={inputOnly ? "输入单价" : "输入价格"}><Input id="model-input-price" inputMode="decimal" value={form.input_price} onChange={event => setForm({ ...form, input_price: event.target.value })} placeholder="例如：2.5" /></Field>
              {!inputOnly && <Field id="model-output-price" label="输出价格"><Input id="model-output-price" inputMode="decimal" value={form.output_price} onChange={event => setForm({ ...form, output_price: event.target.value })} placeholder="例如：10" /></Field>}
            </div>
            : <div className="space-y-4">
              <Field id="model-unit-price" label={`单价（美元／${billingUnitLabels[form.billing_unit]}）`}><Input id="model-unit-price" inputMode="decimal" value={form.unit_price} onChange={event => setForm({ ...form, unit_price: event.target.value })} placeholder="例如：0.04" /></Field>
              {(requiresQuantityField(form.billing_unit) || form.billing_unit === "image") && <Field id="model-quantity-pointer" label={form.billing_unit === "image" ? "实际张数字段（可选）" : "实际用量字段"}><Input id="model-quantity-pointer" value={form.quantity_pointer} onChange={event => setForm({ ...form, quantity_pointer: event.target.value })} placeholder={form.billing_unit === "image" ? "默认统计返回的图片" : form.billing_unit === "second" ? "/usage/duration_seconds" : "/usage/characters"} /></Field>}
              <p className="text-xs leading-5 text-muted-foreground">{form.billing_unit === "image" ? "按返回的实际图片计费；缺少图片结果或张数时未计价。" : form.billing_unit === "request" ? "每次成功返回结果计费；异步任务完成后计费。" : "用量字段使用供应商响应中的 JSON Pointer；未返回实际用量时未计价。"}</p>
            </div>}
            {inputOnly && <p className="text-xs leading-5 text-muted-foreground">按输入 Token 计费，单位为美元／百万 Token。</p>}
            <p className="text-xs leading-5 text-muted-foreground">留空表示未配置，0 表示免费。</p>
            </ModelRouteOptions></>}
          />
          </fieldset>
        </AppForm>
      </FormDialog>

      <ConfirmDialog open={Boolean(deleteTarget)} onOpenChange={open => { if (!open) setDeleteTarget(null) }} title={`删除“${deleteTarget?.name || ""}”？`} description="删除后无法恢复，使用它的客户端将无法再调用该模型。" confirmText="确认删除" cancelText="保留" destructive onConfirm={remove} />
    </div>
  )
}
