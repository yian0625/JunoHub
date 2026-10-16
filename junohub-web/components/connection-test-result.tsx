"use client"

import { Check, CircleAlert, Clock3, X } from "lucide-react"
import { ConnectionTestResult } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"

export function ConnectionTestResultView({ report, name, onUseAddress, addressActionLabel = "使用建议地址" }: { report: ConnectionTestResult; name?: string; onUseAddress?: (url: string) => void; addressActionLabel?: string }) {
  const success = report.outcome === "success"
  const failed = report.outcome === "error"
  const statusLabel = success ? "通过" : failed || report.status_code === 429 || report.status_code >= 500 ? "未通过" : "待验证"
  const Icon = success ? Check : failed ? X : CircleAlert
  return <div role="status" className="min-w-0 space-y-3 py-4">
    <div className="flex items-start gap-3">
      <span className={cn("grid size-8 shrink-0 place-items-center rounded-lg", success ? "bg-success-subtle text-success-text" : failed ? "bg-danger-subtle text-danger-text" : "bg-accent-subtle text-accent-text")}><Icon className="size-4" aria-hidden="true" /></span>
      <div className="min-w-0 flex-1"><div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1"><p className="break-words text-sm font-semibold [overflow-wrap:anywhere]">{name || (success ? "验证通过" : statusLabel === "未通过" ? "本次请求未通过" : "需要进一步验证")}</p>{name && <span className={cn("rounded-md px-2 py-1 text-xs", success ? "bg-success-subtle text-success-text" : failed ? "bg-danger-subtle text-danger-text" : "bg-bg-sunken text-muted-foreground")}>{statusLabel}</span>}</div><p className="mt-1 text-sm leading-6 text-muted-foreground [overflow-wrap:anywhere]">{report.message}</p>
        {(report.status_code > 0 || report.latency_ms !== undefined) && <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs tabular-nums text-muted-foreground">{report.status_code > 0 && <span className="rounded-md bg-bg-sunken px-2 py-1">HTTP {report.status_code}</span>}{report.latency_ms !== undefined && <span className="inline-flex items-center gap-1 rounded-md bg-bg-sunken px-2 py-1"><Clock3 className="size-3" aria-hidden="true" />{report.latency_ms} ms</span>}</div>}
      </div>
    </div>
    {report.request_path && <p className="break-all font-mono text-xs text-muted-foreground">POST {report.request_path}</p>}
    {report.suggested_base_url && <div className="space-y-2 rounded-lg bg-bg-sunken p-3"><p className="text-xs text-muted-foreground">建议 API 地址</p><p className="break-all font-mono text-xs leading-5">{report.suggested_base_url}</p>{onUseAddress ? <Button type="button" variant="outline" size="sm" onClick={() => onUseAddress(report.suggested_base_url!)}>{addressActionLabel}</Button> : <p className="text-xs leading-5 text-muted-foreground">请在对应的连接配置中更新地址后重新测试。</p>}</div>}
  </div>
}
