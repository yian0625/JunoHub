"use client"

import { useEffect, useState } from "react"
import { consoleAPI } from "@/lib/api"
import type { ModelApiConfig } from "@/lib/model-api"

export function ModelRouteDestination({ providerId, model, requestPath, config, invalid }: {
  providerId: number; model: string; requestPath: string; config: ModelApiConfig; invalid: boolean
}) {
  const signature = JSON.stringify({ provider_id: providerId, upstream_model: model.trim(), request_path: requestPath.trim(), api_config: config })
  const [preview, setPreview] = useState<{ signature: string; url?: string; accounts?: number; error?: string } | null>(null)
  useEffect(() => {
    if (!providerId || invalid) return
    let cancelled = false
    const timer = setTimeout(() => {
      consoleAPI.modelPreview(JSON.parse(signature)).then(report => {
        if (!cancelled) setPreview({ signature, url: report.upstream_url || undefined, accounts: report.available_accounts ?? undefined })
      }).catch(error => {
        if (!cancelled) setPreview({ signature, error: error instanceof Error ? error.message : "无法解析上游地址，请稍后重试。" })
      })
    }, 250)
    return () => { cancelled = true; clearTimeout(timer) }
  }, [signature, providerId, invalid])
  const current = preview?.signature === signature ? preview : null
  return <div className="space-y-2 rounded-lg border border-border-default bg-bg-page px-3.5 py-3 text-xs leading-5">
    <p className="font-medium">完整上游地址</p>
    <p className="break-all font-mono">{invalid ? "待完善连接配置" : current?.error || current?.url || "正在解析…"}</p>
    {current?.accounts === 0 && <p className="text-warning-text">没有可调度的账户，请检查供应商账户和模型权限。</p>}
  </div>
}
