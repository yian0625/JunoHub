"use client"

import { useCallback, useEffect, useState } from "react"
import { RefreshCw } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Column, DataTable } from "@/components/data-table"
import { ConsoleLog, consoleAPI } from "@/lib/api"
import { cn } from "@/lib/utils"
import { toast } from "sonner"
import { ErrorState, LoadingState, PageHead } from "../_components/console-ui"
import { useConsoleGuard } from "../_components/use-console-guard"

const PAGE_SIZE = 20

const columns: Column<ConsoleLog>[] = [
  {
    key: "create_time",
    title: "时间",
    render: value => <time className="font-mono text-xs text-muted-foreground">{formatTime(Number(value))}</time>,
  },
  {
    key: "model_alias",
    title: "模型",
    render: (value, row) => <div><p className="text-sm font-medium">{String(value || "-")}</p>{row.upstream_model && <p className="mt-1 font-mono text-xs text-muted-foreground">{row.upstream_model}</p>}</div>,
  },
  {
    key: "provider_type",
    title: "供应商",
    render: value => <span className="font-mono text-xs">{String(value || "-")}</span>,
  },
  {
    key: "status_code",
    title: "状态",
    render: value => {
      const status = Number(value)
      const success = status >= 200 && status < 400
      return <span className={cn("inline-flex min-w-14 justify-center rounded-full px-2.5 py-1 font-mono text-xs font-semibold ring-1 ring-inset", success ? "bg-success-subtle text-success-text ring-success-solid/25" : status > 0 ? "bg-danger-subtle text-danger-text ring-danger-solid/25" : "bg-bg-sunken text-muted-foreground ring-border-default")}>{status || "-"}</span>
    },
  },
  {
    key: "duration_ms",
    title: "耗时",
    render: value => <span className="font-mono text-xs tabular-nums">{formatDuration(Number(value))}</span>,
  },
  {
    key: "error_message",
    title: "错误信息",
    className: "max-w-[20rem]",
    render: value => {
      const message = String(value || "")
      return message ? <span className="block max-w-[20rem] truncate text-xs text-danger-text" title={message}>{message}</span> : <span className="text-xs text-muted-foreground">-</span>
    },
  },
]

export default function LogsPage() {
  const isUser = useConsoleGuard()
  const [logs, setLogs] = useState<ConsoleLog[]>([])
  const [page, setPage] = useState(1)
  const [total, setTotal] = useState(0)
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState("")

  const load = useCallback(async () => {
    setLoading(true); setLoadError("")
    try {
      const data = await consoleAPI.logs(page, PAGE_SIZE)
      setLogs(data.list || []); setTotal(data.total || 0)
    } catch (error) {
      const message = error instanceof Error ? error.message : "无法加载请求日志，请检查网络后重试。"
      setLoadError(message); toast.error(message)
    } finally { setLoading(false) }
  }, [page])

  useEffect(() => { if (isUser) load() }, [isUser, load])

  if (loading && !logs.length) return <LoadingState />
  if (loadError && !logs.length) return <ErrorState message={loadError} onRetry={load} />

  return (
    <div className="view-enter space-y-8">
      <PageHead title="请求日志" description="查看通过网关转发的模型请求、响应状态和处理耗时。" action={<Button variant="outline" onClick={load} disabled={loading}><RefreshCw className={loading ? "animate-spin" : undefined} aria-hidden="true" />刷新</Button>} />
      <DataTable columns={columns} data={logs} page={page} pageSize={PAGE_SIZE} total={total} onPageChange={setPage} loading={loading} />
    </div>
  )
}

function formatTime(timestamp: number) {
  if (!timestamp) return "-"
  return new Date(timestamp * 1000).toLocaleString("zh-CN", { hour12: false })
}

function formatDuration(milliseconds: number) {
  if (milliseconds < 1000) return `${milliseconds} ms`
  return `${(milliseconds / 1000).toFixed(2)} s`
}
