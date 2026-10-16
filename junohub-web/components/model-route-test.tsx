"use client"

import { useEffect, useRef, useState } from "react"
import { RotateCw, Zap } from "lucide-react"
import { Button } from "@/components/coss-ui/button"
import { ConnectionTestResultView } from "@/components/connection-test-result"
import { consoleAPI, type ConnectionTestResult } from "@/lib/api"
import { type ModelApiConfig, upstreamInterface } from "@/lib/model-api"

export function ModelRouteTest({ providerId, model, requestPath, config, disabled = false }: {
  providerId: number; model: string; requestPath: string; config: ModelApiConfig
  disabled?: boolean
}) {
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<{ signature: string; report: ConnectionTestResult } | null>(null)
  const generation = useRef(0)
  const resultView = useRef<HTMLDivElement>(null)
  useEffect(() => () => { generation.current++ }, [])
  const signature = JSON.stringify([providerId, model.trim(), requestPath.trim(), config, disabled])
  useEffect(() => {
    if (result?.signature === signature) resultView.current?.scrollIntoView({ block: "nearest", behavior: "instant" })
  }, [result, signature])
  const supported = ["auto", "chat_completions", "responses", "messages"].includes(upstreamInterface(config).value)
  const test = async () => {
    if (busy || disabled || !supported || !providerId || !model.trim()) return
    const request = ++generation.current
    setBusy(true); setResult(null)
    try {
      const report = await consoleAPI.modelTest({ provider_id: providerId, upstream_model: model.trim(), request_path: requestPath.trim(), api_config: config })
      if (generation.current === request) setResult({ signature, report })
    } catch (error) {
      if (generation.current === request) setResult({ signature, report: { ok: false, outcome: "error", status_code: 0, message: error instanceof Error ? error.message : "上游测试失败，请稍后重试。" } })
    } finally { if (generation.current === request) setBusy(false) }
  }
  return <section className="min-w-0" aria-label="上游模型测试" aria-busy={busy}>
    <Button type="button" variant="outline" disabled={disabled || busy || !providerId || !model.trim() || !supported} onClick={test}>
      {busy ? <RotateCw className="animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Zap aria-hidden="true" />}
      {busy ? "正在测试上游" : "测试上游"}
    </Button>
    <p className="mt-2 text-xs leading-5 text-muted-foreground">{supported ? "按当前配置发送一次短请求，可能产生少量用量。" : "当前仅支持三种对话格式的调用测试。"}</p>
    {busy && <p role="status" className="mt-2 text-xs text-muted-foreground">正在等待上游响应…</p>}
    {result && result.signature !== signature && <p role="status" className="mt-2 text-xs text-muted-foreground">配置已修改，请重新测试。</p>}
    {result?.signature === signature && <div ref={resultView}><ConnectionTestResultView report={result.report} name={result.report.account_name ? `上游测试 · ${result.report.account_name}` : "上游测试"} /></div>}
  </section>
}
