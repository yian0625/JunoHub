"use client"

import { useEffect, useId, useRef, useState } from "react"
import { ArrowRight, RotateCw, Wifi, Zap } from "lucide-react"
import { consoleAPI, ConnectionTestResult } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ConnectionTestResultView } from "@/components/connection-test-result"
import { cn } from "@/lib/utils"
import { Label } from "@/components/ui/label"
import { SearchCombobox } from "@/components/ui/search-combobox"
import { ModelTestSettings, defaultModelTestSettings } from "@/components/model-test-settings"

export function ConnectionTest({ baseUrl, apiKey, platform = "custom", disabled, onUseAddress }: { baseUrl: string; apiKey: string; platform?: string; disabled?: boolean; onUseAddress?: (url: string) => void }) {
  const modelId = useId()
  const [modelOpen, setModelOpen] = useState(false)
  const [model, setModel] = useState("")
  const [manual, setManual] = useState(false)
  const [settings, setSettings] = useState(() => defaultModelTestSettings(platform))
  const [busy, setBusy] = useState<"catalog" | "model" | null>(null)
  const [result, setResult] = useState<{ signature: string; report: ConnectionTestResult } | null>(null)
  const [catalog, setCatalog] = useState<{ connection: string; models: string[] } | null>(null)
  const generation = useRef(0)
  const requestInFlight = useRef(false)
  useEffect(() => () => { generation.current++ }, [])
  const connection = JSON.stringify([baseUrl.trim(), apiKey.trim(), platform])
  const signature = JSON.stringify([connection, modelOpen ? model.trim() : "", modelOpen ? settings : null])
  const models = catalog?.connection === connection ? catalog.models : []
  const report = result?.signature === signature ? result.report : null
  const test = async (callModel: boolean) => {
    if (requestInFlight.current) return
    requestInFlight.current = true
    const request = ++generation.current
    setBusy(callModel ? "model" : "catalog"); setResult(null)
    try {
      const report = await consoleAPI.testConnection({ base_url: baseUrl.trim(), api_key: apiKey.trim(), platform, ...(callModel && model.trim() ? { model: model.trim(), ...(platform === "gemini" ? {} : settings) } : {}) })
      if (generation.current === request) {
        setResult({ signature, report })
        if (!callModel) {
          const models = [...new Set(report.models || [])].sort()
          setCatalog({ connection, models })
          setManual(Boolean(model) && !models.includes(model))
        }
      }
    } catch (error) {
      if (generation.current === request) setResult({ signature, report: { ok: false, outcome: "error", status_code: 0, message: error instanceof Error ? error.message : "测试失败，请稍后重试。" } })
    } finally { requestInFlight.current = false; if (generation.current === request) setBusy(null) }
  }
  return <section className="min-w-0 overflow-hidden rounded-xl border border-border-default bg-bg-sunken" aria-label="连接验证" aria-busy={busy !== null}>
    <div className="flex items-center gap-3 px-4 pt-4"><span className="grid size-9 shrink-0 place-items-center rounded-lg bg-bg-surface text-accent-text"><Wifi className="size-4" aria-hidden="true" /></span><div><h3 className="text-sm font-semibold">连接验证</h3><p className="mt-0.5 text-xs text-muted-foreground">先确认连接，再保存配置</p></div></div>
    <div className="space-y-3 p-4">
      <div className="grid grid-cols-2 gap-1 rounded-lg border border-border-default bg-bg-surface p-1" role="group" aria-label="验证方式">
        {[false, true].map(mode => <button key={String(mode)} type="button" disabled={disabled || busy !== null} aria-pressed={modelOpen === mode} aria-label={mode ? "指定模型验证实际调用" : "返回连接测试"} onClick={() => setModelOpen(mode)} className={cn("flex min-h-10 items-center justify-center gap-2 rounded-md px-2 py-2 text-xs font-medium transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring", modelOpen === mode ? "bg-accent-subtle text-accent-text" : "text-muted-foreground hover:bg-bg-sunken hover:text-foreground")}>
          {mode ? <Zap className="size-3.5" aria-hidden="true" /> : <Wifi className="size-3.5" aria-hidden="true" />}{mode ? "模型调用" : "连接检查"}
        </button>)}
      </div>
      {modelOpen ? <div className="min-w-0 space-y-2">
        {platform !== "gemini" && <ModelTestSettings value={settings} onChange={setSettings} disabled={disabled || busy !== null} prefix={modelId} />}
        <div className="flex flex-wrap items-center justify-between gap-2"><Label htmlFor={modelId}>测试模型</Label><Button type="button" variant="ghost" size="sm" disabled={disabled || busy !== null || !baseUrl.trim()} onClick={() => test(false)}><RotateCw className={busy === "catalog" ? "animate-spin motion-reduce:animate-none" : undefined} aria-hidden="true" />{busy === "catalog" ? "获取中" : models.length ? "刷新模型" : "获取模型"}</Button></div>
        <fieldset disabled={disabled || busy !== null} className="min-w-0 space-y-2"><legend className="sr-only">测试模型</legend>{models.length > 0 && !manual ? <SearchCombobox id={modelId} label="从上游模型列表选择" value={model} onValueChange={setModel} options={models.map(value => ({ value, label: value }))} placeholder="搜索并选择上游模型" /> : <Input id={modelId} value={model} onChange={event => setModel(event.target.value)} placeholder="填写上游模型名称" autoComplete="off" spellCheck={false} />}{models.length > 0 && <div className="flex flex-wrap items-center justify-between gap-2"><p className="text-xs text-muted-foreground">已获取 {models.length} 个模型</p><Button type="button" variant="link" size="sm" className="h-auto px-0 py-1" onClick={() => { if (manual && !models.includes(model)) setModel(""); setManual(value => !value) }}>{manual ? "返回模型列表" : "手动填写模型 ID"}</Button></div>}</fieldset>
        <p className="text-xs leading-5 text-muted-foreground">发送一次简短请求，可能产生少量用量。</p>
      </div> : <p className="text-xs leading-5 text-muted-foreground">检查地址、凭证及模型列表，不发送生成请求。</p>}
      <Button type="button" size="sm" className="w-full" disabled={disabled || busy !== null || !baseUrl.trim() || (modelOpen && !model.trim())} onClick={() => test(modelOpen)}>
        {busy === (modelOpen ? "model" : "catalog") ? <RotateCw className="animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <ArrowRight aria-hidden="true" />}{busy === (modelOpen ? "model" : "catalog") ? "正在测试" : modelOpen ? "验证模型调用" : "测试连接"}
      </Button>
      {!baseUrl.trim() && <p className="text-xs text-muted-foreground">填写 API 地址后即可测试</p>}
      {busy !== null && <p role="status" className="text-xs leading-5 text-muted-foreground">{busy === "catalog" && modelOpen ? "正在获取上游模型列表…" : "正在等待上游响应，请稍候…"}</p>}
    </div>
    {!modelOpen && models.length > 0 && <div className="space-y-2 px-4 pb-4"><p className="text-xs text-muted-foreground">已获取 {models.length} 个模型</p><fieldset disabled={disabled || busy !== null}><legend className="sr-only">已获取的上游模型</legend><SearchCombobox id={`${modelId}-catalog`} label="从上游模型列表选择" value={model} onValueChange={value => { setModel(value); setModelOpen(true) }} options={models.map(value => ({ value, label: value }))} placeholder="搜索并选择模型进行验证" /></fieldset></div>}
    {report && <div className="border-t border-border-default bg-bg-surface px-4"><ConnectionTestResultView report={report} onUseAddress={onUseAddress} /></div>}
  </section>
}
