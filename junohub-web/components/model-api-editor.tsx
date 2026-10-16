"use client"

import { useRef, useSyncExternalStore, type ReactNode } from "react"
import { Input } from "@/components/coss-ui/input"
import { SelectField } from "@/components/ui/select-field"
import { Field, FormError } from "@/app/(admin)/console/_components/console-ui"
import { ModelRouteOptions } from "@/components/model-route-options"
import { ModelRouteDestination } from "@/components/model-route-destination"
import { ModelRouteTest } from "@/components/model-route-test"
import { Context1MSetting } from "@/components/context-1m-setting"
import { gatewayProtocolOptions, modelInterface, ModelApiConfig, routeConfigError, upstreamInterface, upstreamFormatValue, upstreamFormatOptions, selectUpstreamFormat, selectGatewayProtocol, normalizePastedUpstreamPath } from "@/lib/model-api"

const subscribeOrigin = () => () => {}
const browserOrigin = () => window.location.origin
const serverOrigin = () => ""

export function ModelApiEditor({ config, requestPath, managed, providerId, providerBaseUrl, upstreamModel, upstreamFields, gatewayFields, gatewayOptions, onChange }: {
  config: ModelApiConfig; requestPath: string; managed: boolean
  upstreamFields: ReactNode; gatewayFields: ReactNode
  gatewayOptions?: ReactNode
  providerId: number; providerBaseUrl: string; upstreamModel: string
  onChange: (config: ModelApiConfig, path: string) => void
}) {
  const gateway = modelInterface(config.api_type)
  const upstream = upstreamInterface(config)
  const dialogue = ["auto", "chat_completions", "responses", "messages"].includes(upstream.value)
  const formatValue = upstreamFormatValue(config, requestPath)
  const custom = formatValue.startsWith("custom:")
  const customPath = useRef(requestPath)
  const gatewayOrigin = useSyncExternalStore(subscribeOrigin, browserOrigin, serverOrigin)
  const error = routeConfigError(config, requestPath, managed)

  return <div className="grid min-w-0 gap-6 min-[800px]:grid-cols-2 min-[800px]:gap-8">
    <fieldset className="min-w-0 space-y-4">
      <legend className="mb-3 text-sm font-semibold">调用上游平台</legend>
      {upstreamFields}
      {managed ? <p className="text-xs leading-5 text-muted-foreground">平台授权池由账户引擎管理上游协议、鉴权和路径。</p> : <>
        <Field id="model-upstream-api-type" label="上游请求格式">
          <SelectField id="model-upstream-api-type" value={formatValue} options={upstreamFormatOptions(config, requestPath)} aria-invalid={error?.field === "model-upstream-api-type"} aria-describedby={error?.field === "model-upstream-api-type" ? "model-config-error" : undefined} onValueChange={value => {
            if (custom) customPath.current = requestPath
            const next = selectUpstreamFormat(config, value)
            onChange(next, next.upstream_mode === "custom" ? customPath.current : "")
          }} />
        </Field>
        {custom && <Field id="model-request-path" label="上游请求路径">
          <Input id="model-request-path" name="request-path" value={requestPath} aria-invalid={error?.field === "model-request-path"} aria-describedby={error?.field === "model-request-path" ? "model-config-error" : undefined} onChange={event => onChange(config, event.target.value)} onPaste={event => {
            const pasted = event.clipboardData.getData("text")
            const normalized = normalizePastedUpstreamPath(pasted, providerBaseUrl)
            if (normalized !== pasted) { event.preventDefault(); onChange(config, normalized) }
          }} onBlur={event => {
            const normalized = normalizePastedUpstreamPath(event.target.value, providerBaseUrl)
            if (normalized !== event.target.value) onChange(config, normalized)
          }} placeholder="/your/api/path" spellCheck={false} required />
        </Field>}
        <ModelRouteDestination providerId={providerId} model={upstreamModel} requestPath={requestPath} config={config} invalid={Boolean(error)} />
        <ModelRouteOptions label="高级设置" summary={`${config.auth_type === "bearer" ? "Bearer" : "X-API-Key"} · ${config.response_mode === "task" ? "异步任务" : "直接响应"}${config.context_1m ? " · 1M" : ""}`} defaultOpen={config.auth_type !== "bearer" || config.response_mode === "task" || Boolean(config.context_1m)}>
          <Field id="model-api-auth" label="上游鉴权"><SelectField id="model-api-auth" value={config.auth_type} options={[{ value: "bearer", label: "Authorization · Bearer" }, { value: "x_api_key", label: "X-API-Key" }]} onValueChange={value => onChange({ ...config, auth_type: value as ModelApiConfig["auth_type"] }, requestPath)} /></Field>
          <Field id="model-api-response" label="上游返回方式"><SelectField id="model-api-response" value={config.response_mode} options={[{ value: "direct", label: "直接响应（JSON / 流式 / 文件）" }, { value: "task", label: "任务 ID · 另行查询", disabled: dialogue }]} onValueChange={value => onChange({ ...config, response_mode: value as ModelApiConfig["response_mode"], task_path: value === "direct" ? "" : config.task_path }, requestPath)} /></Field>
          {upstream.value === "messages" && <Context1MSetting id="model-context-1m" checked={Boolean(config.context_1m)} onCheckedChange={context_1m => onChange({ ...config, context_1m }, requestPath)} />}
        </ModelRouteOptions>
        {config.response_mode === "task" && <Field id="model-task-path" label="上游任务查询路径">
          <Input id="model-task-path" value={config.task_path} onChange={event => onChange({ ...config, task_path: event.target.value }, requestPath)} placeholder="填写查询路径，包含 {task_id}" spellCheck={false} required />
          <p className="mt-1.5 text-xs leading-5 text-muted-foreground">用 {"{task_id}"} 标记供应商返回的任务 ID。</p>
        </Field>}
      </>}
      <ModelRouteTest providerId={providerId} model={upstreamModel} requestPath={requestPath} config={config} disabled={Boolean(error)} />
    </fieldset>

    <fieldset className="min-w-0 space-y-4 border-t border-border-default pt-5 min-[800px]:border-s min-[800px]:border-t-0 min-[800px]:ps-8 min-[800px]:pt-0">
      <legend className="mb-3 pe-2 text-sm font-semibold">通过 JunoHub 调用</legend>
      {gatewayFields}
      <Field id="model-api-type" label="JunoHub 预设接口">
        <SelectField id="model-api-type" value={config.api_type} options={gatewayProtocolOptions(config, managed)} onValueChange={value => onChange(selectGatewayProtocol(config, value as ModelApiConfig["api_type"], requestPath, managed), requestPath)} />
      </Field>
      <div className="space-y-2 rounded-lg border border-border-default bg-bg-page px-3.5 py-3 text-xs leading-5">
        <p className="font-medium">客户端调用地址</p>
        {config.api_type === "auto" ? <>
          <p className="break-all font-mono">POST {gatewayOrigin}/v1/chat/completions</p>
          {managed && <p className="break-all font-mono">POST {gatewayOrigin}/v1/responses</p>}
        </> : <p className="break-all font-mono">POST {gatewayOrigin}{gateway.endpoint}</p>}
        {config.response_mode === "task" && <p className="break-all font-mono">GET {gatewayOrigin}/v1/tasks/{"{task_id}"}</p>}
      </div>
      {error && <div id="model-config-error" role="alert"><FormError message={error.message} /></div>}
      {gatewayOptions}
    </fieldset>
  </div>
}
