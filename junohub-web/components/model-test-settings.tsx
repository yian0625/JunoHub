"use client"

import { Label } from "@/components/ui/label"
import { SelectField } from "@/components/ui/select-field"
import { Context1MSetting } from "@/components/context-1m-setting"

export type ModelTestSettingsValue = { api_type: "chat_completions" | "responses" | "messages"; auth_type: "bearer" | "x_api_key"; context_1m?: boolean }
export function defaultModelTestSettings(platform: string): ModelTestSettingsValue {
  return platform === "anthropic" ? { api_type: "messages", auth_type: "x_api_key" } : { api_type: "chat_completions", auth_type: "bearer" }
}

export function ModelTestSettings({ value, onChange, disabled, prefix }: { value: ModelTestSettingsValue; onChange: (value: ModelTestSettingsValue) => void; disabled?: boolean; prefix: string }) {
  return <fieldset disabled={disabled} className="grid min-w-0 grid-cols-1 gap-3 min-[480px]:grid-cols-2"><legend className="sr-only">模型测试请求配置</legend>
    <div className="min-w-0 space-y-2"><Label htmlFor={`${prefix}-protocol`}>请求格式</Label><SelectField id={`${prefix}-protocol`} value={value.api_type} disabled={disabled} options={[{ value: "chat_completions", label: "Chat Completions" }, { value: "responses", label: "Responses" }, { value: "messages", label: "Messages" }]} onValueChange={api_type => onChange({ ...value, api_type: api_type as ModelTestSettingsValue["api_type"], context_1m: api_type === "messages" && Boolean(value.context_1m) })} /></div>
    <div className="min-w-0 space-y-2"><Label htmlFor={`${prefix}-auth`}>鉴权方式</Label><SelectField id={`${prefix}-auth`} value={value.auth_type} disabled={disabled} options={[{ value: "bearer", label: "Authorization · Bearer" }, { value: "x_api_key", label: "X-API-Key" }]} onValueChange={auth_type => onChange({ ...value, auth_type: auth_type as ModelTestSettingsValue["auth_type"] })} /></div>
    {value.api_type === "messages" && <div className="min-[480px]:col-span-2"><Context1MSetting id={`${prefix}-context-1m`} checked={Boolean(value.context_1m)} disabled={disabled} onCheckedChange={context_1m => onChange({ ...value, context_1m })} /></div>}
  </fieldset>
}
