"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { ChevronLeft, ChevronRight, Eye, RefreshCw, RotateCcw, SearchIcon } from "lucide-react"
import { Button } from "@/components/coss-ui/button"
import { Badge } from "@/components/coss-ui/badge"
import { Combobox, ComboboxEmpty, ComboboxInput, ComboboxItem, ComboboxList, ComboboxPopup } from "@/components/coss-ui/combobox"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/coss-ui/input-group"
import { Table, TableBody, TableCaption, TableCell, TableHead, TableHeader, TableRow } from "@/components/coss-ui/table"
import { Card } from "@/components/ui/card"
import { Spinner } from "@/components/ui/spinner"
import { DateRangeFilter } from "@/components/date-range-filter"
import { ConsoleLog, consoleAPI } from "@/lib/api"
import { billingUnitLabels } from "@/lib/model-billing"
import { toast } from "sonner"
import { ErrorState, FormDialog, LoadingState, PageHead } from "../_components/console-ui"
import { useConsoleGuard } from "../_components/use-console-guard"

const PAGE_SIZE = 20
type StatusValue = "success" | "4xx" | "5xx" | "other" | ""
type StatusOption = { value: StatusValue; label: string }
const statusOptions: StatusOption[] = [
  { value: "", label: "全部状态" },
  { value: "success", label: "成功" },
  { value: "4xx", label: "4xx 客户端错误" },
  { value: "5xx", label: "5xx 服务端错误" },
  { value: "other", label: "其他状态" },
]

interface LogFilterState {
  requestId: string;
  user: string
  model: string
  provider: string
  status: StatusValue
  startDate: string
  endDate: string
}

const emptyFilters: LogFilterState = { requestId: "", user: "", model: "", provider: "", status: "", startDate: "", endDate: "" }

export default function LogsPage() {
  const isUser = useConsoleGuard()
  const [logs, setLogs] = useState<ConsoleLog[]>([])
  const [page, setPage] = useState(1)
  const [total, setTotal] = useState(0)
  const [scope, setScope] = useState<"all" | "own">("own")
  const [filters, setFilters] = useState<LogFilterState>(emptyFilters)
  const [loading, setLoading] = useState(true)
  const [initialized, setInitialized] = useState(false)
  const [loadError, setLoadError] = useState("")
  const [selectedLog, setSelectedLog] = useState<ConsoleLog | null>(null)
  const loadSequence = useRef(0)

  const load = useCallback(async () => {
    const sequence = ++loadSequence.current
    setLoading(true); setLoadError("")
    try {
      const data = await consoleAPI.logs({
        page,
        page_size: PAGE_SIZE,
        ...(filters.requestId ? { request_id: filters.requestId.trim() } : {}),
        ...(filters.user ? { user_search: filters.user } : {}),
        ...(filters.model ? { model: filters.model } : {}),
        ...(filters.provider ? { provider: filters.provider } : {}),
        ...(filters.status ? { status: filters.status } : {}),
        ...(filters.startDate ? { from: dateStart(filters.startDate) } : {}),
        ...(filters.endDate ? { to: dateEnd(filters.endDate) } : {}),
      })
      if (sequence !== loadSequence.current) return
      setLogs(data.list || []); setTotal(data.total || 0); setScope(data.scope || "own")
    } catch (error) {
      if (sequence !== loadSequence.current) return
      const message = error instanceof Error ? error.message : "无法加载请求日志，请检查网络后重试。"
      setLoadError(message); toast.error(message)
    } finally { if (sequence === loadSequence.current) { setLoading(false); setInitialized(true) } }
  }, [filters, page])

  useEffect(() => { if (isUser) void load() }, [isUser, load])
  if (!initialized && loading) return <LoadingState />
  if (!initialized && loadError) return <ErrorState message={loadError} onRetry={load} />

  const updateFilter = <K extends keyof LogFilterState>(key: K, value: LogFilterState[K]) => {
    setFilters(current => ({ ...current, [key]: value })); setPage(1)
  }
  const hasSearch = Object.values(filters).some(Boolean)
  const pageCount = Math.max(1, Math.ceil(total / PAGE_SIZE))
  const admin = scope === "all"
  const columnCount = 9

  return (
    <div className="view-enter space-y-6">
      <PageHead title="请求日志" description={admin ? "查询网关请求，排查模型、供应商和调用错误。" : "查看你自己的模型请求、Token 用量和调用结果。"} action={<Button size="icon" variant="outline" aria-label="刷新日志" title="刷新日志" onClick={load} disabled={loading}><RefreshCw className={loading ? "animate-spin" : undefined} aria-hidden="true" /></Button>} />

      <section className="space-y-4" aria-label="请求日志查询">
        <div role="region" aria-label="筛选请求日志" className="space-y-3">
          <div className="flex flex-wrap items-center gap-3">
            <LogSearchFilter label="请求 ID" value={filters.requestId} onChange={value => updateFilter("requestId", value)} className="min-w-[12rem] flex-1" />
            <LogSearchFilter label="用户" value={filters.user} onChange={value => updateFilter("user", value)} className="min-w-[12rem] flex-[1.15]" />
            <LogSearchFilter label="模型" value={filters.model} onChange={value => updateFilter("model", value)} className="min-w-[11rem] flex-1" />
            <LogSearchFilter label="供应商" value={filters.provider} onChange={value => updateFilter("provider", value)} className="min-w-[11rem] flex-1" />
            <div className="min-w-[11rem] flex-1">
              <Combobox<StatusOption> items={statusOptions} value={statusOptions.find(item => item.value === filters.status) || statusOptions[0]} onValueChange={item => updateFilter("status", item?.value || "")} itemToStringLabel={item => item.label} itemToStringValue={item => item.value}>
                <ComboboxInput aria-label="状态" placeholder="全部状态" />
                <ComboboxPopup>
                  <ComboboxEmpty>没有匹配的状态</ComboboxEmpty>
                  <ComboboxList>
                    {item => <ComboboxItem key={item.value} value={item}>{item.label}</ComboboxItem>}
                  </ComboboxList>
                </ComboboxPopup>
              </Combobox>
            </div>
            <div className="min-w-[12rem] flex-1"><DateRangeFilter startDate={filters.startDate} endDate={filters.endDate} onApply={(startDate, endDate) => { setFilters(current => ({ ...current, startDate, endDate })); setPage(1) }} /></div>
          </div>
          {hasSearch && <div className="flex justify-end"><Button variant="ghost" size="sm" onClick={() => { setFilters(emptyFilters); setPage(1) }}><RotateCcw aria-hidden="true" />重置搜索</Button></div>}
        </div>

        <Card className="gap-0 overflow-hidden py-0">
          <Table className="min-w-[1120px]" render={<div role="region" aria-label="请求日志表格，可横向滚动" tabIndex={0} className="overscroll-x-contain focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring" />}>
            <TableCaption className="sr-only">请求日志，按时间从新到旧排列</TableCaption>
            <TableHeader>
              <TableRow className="bg-bg-sunken/60">
                <TableHead scope="col" className="min-w-40 px-4">时间</TableHead>
                <TableHead scope="col" className="min-w-44 px-4">用户</TableHead>
                <TableHead scope="col" className="min-w-32 px-4">Key 名</TableHead>
                <TableHead scope="col" className="min-w-40 px-4">请求模型</TableHead>
                <TableHead scope="col" className="min-w-36 px-4">供应商</TableHead>
                <TableHead scope="col" className="min-w-32 px-4">状态</TableHead>
                <TableHead scope="col" className="text-right px-4">耗时</TableHead>
                <TableHead scope="col" className="text-right px-4">Token</TableHead>
                <TableHead scope="col" className="w-16 text-right px-4">详情</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {loading ? (
                <TableRow><TableCell colSpan={columnCount} className="h-32 text-center"><span className="inline-flex items-center gap-2 text-muted-foreground" role="status"><Spinner className="size-4" />正在加载</span></TableCell></TableRow>
              ) : logs.length === 0 ? (
                <TableRow><TableCell colSpan={columnCount} className="h-36 text-center"><p className="font-medium">{hasSearch ? "没有匹配的请求" : "暂无请求记录"}</p><p className="mt-2 text-sm text-muted-foreground">{hasSearch ? "调整搜索内容、状态或日期后重试。" : "使用访问密钥发起模型请求后，记录会显示在这里。"}</p></TableCell></TableRow>
              ) : logs.map(log => (
                <TableRow key={log.id}>
                  <TableCell className="px-4 py-3.5"><time className="font-mono text-xs text-muted-foreground">{formatTime(log.create_time)}</time></TableCell>
                  <TableCell className="px-4 py-3.5"><p className="font-medium">{log.owner_name || log.owner_email || "未命名用户"}</p>{log.owner_email && <p className="mt-1 text-xs text-muted-foreground">{log.owner_email}</p>}</TableCell>
                  <TableCell className="px-4 py-3.5 font-medium">{log.key_name || "已删除密钥"}</TableCell>
                  <TableCell className="px-4 py-3.5 font-mono text-xs">{log.model_alias || "-"}</TableCell>
                  <TableCell className="px-4 py-3.5">{log.provider_name || log.provider_type || "-"}</TableCell>
                  <TableCell className="px-4 py-3.5"><StatusBadge status={log.status_code} /></TableCell>
                  <TableCell className="px-4 py-3.5 text-right font-mono text-xs tabular-nums">{formatDuration(log.duration_ms)}</TableCell>
                  <TableCell className="px-4 py-3.5 text-right font-mono text-xs tabular-nums">{log.total_tokens.toLocaleString("zh-CN")}</TableCell>
                  <TableCell className="px-4 py-3.5 text-right"><Button size="icon-sm" variant="ghost" aria-label={`查看 ${log.model_alias} 请求详情`} title="查看详情" onClick={() => setSelectedLog(log)}><Eye aria-hidden="true" /></Button></TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
          <p className="border-t border-border-default px-4 py-2 text-xs text-muted-foreground sm:hidden">左右滑动查看完整表格</p>
          <div className="flex flex-wrap items-center justify-between gap-3 border-t border-border-default px-4 py-3">
            <span role="status" className="text-xs text-muted-foreground tabular-nums">共 {total} 条{pageCount > 1 && `，第 ${page} / ${pageCount} 页`}</span>
            {pageCount > 1 && <div className="flex gap-2"><Button size="sm" variant="outline" onClick={() => setPage(current => current - 1)} disabled={page <= 1}><ChevronLeft aria-hidden="true" />上一页</Button><Button size="sm" variant="outline" onClick={() => setPage(current => current + 1)} disabled={page >= pageCount}>下一页<ChevronRight aria-hidden="true" /></Button></div>}
          </div>
        </Card>
      </section>

      <FormDialog open={Boolean(selectedLog)} onOpenChange={open => { if (!open) setSelectedLog(null) }} title="请求详情" description={selectedLog ? formatTime(selectedLog.create_time) : ""} contentClassName="sm:max-w-2xl">
        {selectedLog && <LogDetails log={selectedLog} />}
      </FormDialog>
    </div>
  )
}

function LogSearchFilter({ label, value, onChange, className }: { label: string; value: string; onChange: (value: string) => void; className?: string }) {
  return <div className={className}>
    <InputGroup>
      <InputGroupInput type="search" aria-label={`搜索${label}`} placeholder={`搜索${label}`} value={value} onChange={event => onChange(event.target.value)} />
      <InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon>
    </InputGroup>
  </div>
}

function StatusBadge({ status }: { status: number }) {
  const success = status >= 200 && status < 400
  const clientError = status >= 400 && status < 500
  const serverError = status >= 500
  const label = success ? "成功" : clientError ? "客户端错误" : serverError ? "服务端错误" : "其他"
  const dot = success ? "bg-success-solid" : clientError ? "bg-warning-solid" : serverError ? "bg-danger-solid" : "bg-muted-foreground/60"
  return <Badge variant="outline" className="gap-1.5 px-2"><span aria-hidden="true" className={`size-1.5 rounded-full ${dot}`} /><span className="font-mono tabular-nums">{status || "-"}</span>{label}</Badge>
}

function LogDetails({ log }: { log: ConsoleLog }) {
  const bill = log.billing_details
  const unpricedReasons: Record<string, string> = { missing_usage: "供应商未返回完整 Token 用量", missing_quantity: "供应商未返回实际计费用量", missing_price: "未配置单价", no_successful_result: "尚未获得成功结果", invalid_quantity: "供应商返回的用量格式无效", amount_out_of_range: "计费金额超出范围" }
  const items = [
    ["用户", log.owner_name || log.owner_email || "未命名用户"], ["邮箱", log.owner_email || "-"],
    ["Key 名", log.key_name || "已删除密钥"],
    ["请求模型", log.model_alias || "-"],
    ["上游模型", log.upstream_model || "-"], ["供应商", log.provider_name || log.provider_type || "-"],
    ["耗时", formatDuration(log.duration_ms)],
    ["请求 ID", log.request_id || "历史请求未记录"],
  ]
  return <div className="space-y-5">
    <dl className="grid gap-x-8 gap-y-4 sm:grid-cols-2">
      {items.map(([label, value]) => <div key={label} className="min-w-0"><dt className="text-xs font-medium text-muted-foreground">{label}</dt><dd className="mt-1 break-words text-sm font-medium">{value}</dd></div>)}
      <div><dt className="text-xs font-medium text-muted-foreground">状态</dt><dd className="mt-1"><StatusBadge status={log.status_code} /></dd></div>
      <div><dt className="text-xs font-medium text-muted-foreground">Token</dt><dd className="mt-1 font-mono text-sm tabular-nums">{log.total_tokens.toLocaleString("zh-CN")}</dd><p className="mt-1 text-xs text-muted-foreground">输入 {log.input_tokens.toLocaleString("zh-CN")} · 输出 {log.output_tokens.toLocaleString("zh-CN")}</p></div>
      <div><dt className="text-xs font-medium text-muted-foreground">费用</dt><dd className="mt-1 font-mono text-sm tabular-nums">{log.cost_usd == null ? "未计价" : `$${bill?.amount_usd || log.cost_usd.toLocaleString("en-US", { maximumFractionDigits: 10 })}`}</dd>{log.cost_usd == null && <p className="mt-1 text-xs text-muted-foreground">{unpricedReasons[bill?.unpriced_reason || ""] || "缺少价格或实际用量"}</p>}</div>
      {bill?.unit && bill.unit !== "token" && bill.quantity !== undefined && <div><dt className="text-xs font-medium text-muted-foreground">计费用量</dt><dd className="mt-1 font-mono text-sm tabular-nums">{bill.quantity} {billingUnitLabels[bill.unit]}</dd>{bill.price_per_unit !== undefined && <p className="mt-1 text-xs text-muted-foreground">单价 ${bill.price_per_unit}／{billingUnitLabels[bill.unit]}</p>}</div>}
    </dl>
    {log.error_message && <div className="rounded-xl bg-danger-subtle px-4 py-3"><p className="text-xs font-semibold text-danger-text">错误信息</p><p className="mt-1 whitespace-pre-wrap break-words font-mono text-xs leading-5 text-danger-text">{log.error_message}</p></div>}
  </div>
}

function dateStart(value: string) { return Math.floor(new Date(`${value}T00:00:00`).getTime() / 1000) }
function dateEnd(value: string) { const date = new Date(`${value}T00:00:00`); date.setDate(date.getDate() + 1); return Math.floor(date.getTime() / 1000) }
function formatTime(timestamp: number) { return timestamp ? new Date(timestamp * 1000).toLocaleString("zh-CN", { hour12: false }) : "-" }
function formatDuration(milliseconds: number) { return milliseconds < 1000 ? `${milliseconds} ms` : `${(milliseconds / 1000).toFixed(2)} s` }
