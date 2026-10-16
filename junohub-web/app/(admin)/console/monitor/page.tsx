"use client"

import Link from "next/link"
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react"
import { Activity, Bell, ChevronLeft, ChevronRight, ExternalLink, Settings2 } from "lucide-react"
import { Button } from "@/components/coss-ui/button"
import { Badge } from "@/components/coss-ui/badge"
import { Input } from "@/components/coss-ui/input"
import { Card } from "@/components/ui/card"
import { Switch } from "@/components/ui/switch"
import { SelectField } from "@/components/ui/select-field"
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs"
import { Table, TableHeader, TableRow, TableHead, TableBody, TableCell, TableCaption } from "@/components/coss-ui/table"
import { AppForm } from "@/components/ui/app-form"
import { monitorAPI } from "@/lib/api"
import { useAuthStore } from "@/lib/store"
import { auditActionLabel, monitorStatusLabels, monitorWorkerLabels, type AlertConfig, type MonitorOverview, type MonitorAlert, type MonitorTask, type MonitorAudit, type MonitorList, type MonitorSettings } from "@/lib/monitoring"
import { ErrorState, Field, FormDialog, FormError, LoadingState, PageHead } from "../_components/console-ui"
import { toast } from "sonner"

type Tab = "overview" | "alerts" | "tasks" | "audit"
const number = (value: number | null | undefined) => value == null ? "—" : value.toLocaleString("zh-CN")
const date = (timestamp: number) => timestamp ? new Date(timestamp * 1000).toLocaleString("zh-CN", { hour12: false }) : "—"
const rate = (value: number | null) => value == null ? "—" : `${value.toFixed(1)}%`
const duration = (value: number | null | undefined) => value == null ? "—" : value >= 1000 ? `${(value / 1000).toFixed(2)}s` : `${value}ms`

function Status({ value, label }: { value: string; label?: string }) {
  const good = ["healthy", "eligible", "succeeded", "resolved"].includes(value)
  const bad = ["unavailable", "failed", "submit_failed", "critical"].includes(value)
  const muted = ["disabled", "unknown"].includes(value)
  return <Badge variant="outline" className="gap-1.5"><span aria-hidden="true" className={`size-1.5 shrink-0 rounded-full ${good ? "bg-success-solid" : bad ? "bg-danger-solid" : muted ? "bg-muted-foreground/60" : "bg-warning-solid"}`} />{label || monitorStatusLabels[value] || value}</Badge>
}
function Panel({ title, note, children }: { title: string; note?: string; children: ReactNode }) {
  return <Card className="min-w-0 gap-0 py-0"><div className="p-5"><h2 className="text-sm font-semibold">{title}</h2>{note && <p className="mt-1.5 text-xs leading-6 text-muted-foreground">{note}</p>}</div>{children}</Card>
}
function ScrollTable({ label, headers, children, minWidth = "min-w-[620px]" }: { label: string; headers: string[]; children: ReactNode; minWidth?: string }) {
  return <div tabIndex={0} role="region" aria-label={`${label}，可横向滚动`} className="overflow-x-auto focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-ring"><Table className={minWidth}><TableCaption className="sr-only">{label}</TableCaption><TableHeader><TableRow className="bg-bg-sunken/60">{headers.map(title => <TableHead key={title} scope="col" className="whitespace-nowrap px-4 py-3">{title}</TableHead>)}</TableRow></TableHeader><TableBody>{children}</TableBody></Table></div>
}
function EmptyRow({ columns, children }: { columns: number; children: ReactNode }) { return <TableRow><TableCell colSpan={columns} className="px-4 py-10 text-center text-sm text-muted-foreground">{children}</TableCell></TableRow> }
function Footer({ page, total, onPageChange, busy }: { page: number; total: number; onPageChange: (page: number) => void; busy: boolean }) {
  const pages = Math.max(1, Math.ceil(total / 20))
  return <div className="flex flex-wrap items-center justify-between gap-3 border-t border-border-default px-4 py-3"><p role="status" className="text-xs text-muted-foreground">共 {number(total)} 条{pages > 1 && ` · 第 ${page} / ${pages} 页`}</p>{pages > 1 && <div className="flex gap-2"><Button size="sm" variant="outline" disabled={busy || page <= 1} onClick={() => onPageChange(page - 1)}><ChevronLeft aria-hidden="true" />上一页</Button><Button size="sm" variant="outline" disabled={busy || page >= pages} onClick={() => onPageChange(page + 1)}>下一页<ChevronRight aria-hidden="true" /></Button></div>}</div>
}

export default function MonitoringPage() {
  const admin = useAuthStore(state => state.user?.role === "admin")
  const [tab, setTab] = useState<Tab>("overview")
  const [hours, setHours] = useState("24")
  const [auto, setAuto] = useState(true)
  const [page, setPage] = useState(1)
  const [alertScope, setAlertScope] = useState<"active" | "history">("active")
  const [taskStatus, setTaskStatus] = useState("all")
  const [data, setData] = useState<MonitorOverview | null>(null)
  const [alerts, setAlerts] = useState<MonitorList<MonitorAlert> | null>(null)
  const [tasks, setTasks] = useState<MonitorList<MonitorTask> | null>(null)
  const [audit, setAudit] = useState<MonitorList<MonitorAudit> | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const [action, setAction] = useState<string | null>(null)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const generation = useRef(0)
  const load = useCallback(async () => {
    if (!admin) return
    const current = ++generation.current
    setBusy(true); setError("")
    const results = await Promise.allSettled([monitorAPI.overview(Number(hours)), tab === "alerts" ? monitorAPI.alerts(alertScope, page) : tab === "tasks" ? monitorAPI.tasks(taskStatus === "all" ? "" : taskStatus, page) : tab === "audit" ? monitorAPI.audit(page) : Promise.resolve(null)])
    if (current !== generation.current) return
    if (results[0].status === "fulfilled") setData(results[0].value)
    const table = results[1]
    if (table.status === "fulfilled" && table.value) {
      if (tab === "alerts") setAlerts(table.value as MonitorList<MonitorAlert>)
      if (tab === "tasks") setTasks(table.value as MonitorList<MonitorTask>)
      if (tab === "audit") setAudit(table.value as MonitorList<MonitorAudit>)
    }
    const failed = results.find(result => result.status === "rejected")
    if (failed?.status === "rejected") setError(failed.reason instanceof Error ? failed.reason.message : "监控数据加载失败，请重试。")
    setBusy(false)
  }, [admin, hours, tab, page, alertScope, taskStatus])
  const invalidate = useCallback(() => { generation.current++ }, [])
  useEffect(() => { void load(); return invalidate }, [load, invalidate])
  useEffect(() => {
    if (!auto) return
    const timer = setInterval(() => { if (document.visibilityState === "visible") void load() }, 30_000)
    return () => clearInterval(timer)
  }, [auto, load])
  const mutate = async (id: string, operation: () => Promise<unknown>, success: string) => {
    if (action) return
    setAction(id)
    try { await operation(); toast.success(success); await load() }
    catch (error) { toast.error(error instanceof Error ? error.message : "操作失败，请重试。") }
    finally { setAction(null) }
  }
  if (!admin) return <LoadingState />
  if (!data && error) return <ErrorState message={error} onRetry={load} />
  if (!data) return <LoadingState />
  const s = data.summary
  const cell = "px-4 py-3.5 text-xs leading-6"
  return <div className="space-y-8">
    <PageHead title="运行监控" description="查看调用表现、上游调度与后台任务。" action={<Button variant="outline" onClick={() => setSettingsOpen(true)}><Settings2 aria-hidden="true" />告警规则</Button>} />
    <div className="flex flex-wrap items-center gap-3">
      <div className="w-40"><label className="sr-only" htmlFor="monitor-range">统计范围</label><SelectField id="monitor-range" size="search" value={hours} onValueChange={setHours} options={[{ value: "1", label: "最近 1 小时" }, { value: "24", label: "最近 24 小时" }, { value: "168", label: "最近 7 天" }]} /></div>
      <Button size="sm" variant="outline" loading={busy} onClick={() => void load()}>刷新数据</Button>
      <label htmlFor="monitor-auto" className="flex items-center gap-2 text-xs text-muted-foreground"><Switch id="monitor-auto" checked={auto} onCheckedChange={setAuto} />每 30 秒更新</label>
      <p className="text-xs text-muted-foreground sm:ms-auto">数据时间：{date(data.generated_at)}</p>
    </div>
    {error && <p role="alert" className="rounded-xl border border-danger-text/20 bg-danger-subtle p-4 text-sm text-danger-text">刷新失败：{error}。正在显示上次成功读取的数据。</p>}
    {data.snapshot.stale && <p role="status" className="rounded-xl border border-border-default bg-bg-sunken p-4 text-sm">运行状态尚未采集或已超过 90 秒未更新，旧状态仅供参考。调用统计仍来自数据库。</p>}
    <div className="grid gap-4 min-[600px]:grid-cols-2 xl:grid-cols-4">
      {[
        { label: "调用请求", value: number(s.requests), note: `${number(s.pending)} 次待处理 · ${number(s.cancelled)} 次取消` },
        { label: "完成成功率", value: rate(s.success_rate), note: `${number(s.success)} 次成功 · ${number(s.failed)} 次失败，不含待处理和取消` },
        { label: "P95 成功耗时", value: duration(s.p95_ms), note: `P50 ${duration(s.p50_ms)} · 含生成与响应传输` },
        { label: "已计价费用", value: `$${s.cost_usd.toLocaleString("en-US", { maximumFractionDigits: 6 })}`, note: `${number(s.unpriced)} 次成功请求未计价` },
      ].map(item => <Card key={item.label} className="gap-0 p-5"><p className="text-xs text-muted-foreground">{item.label}</p><p className="mt-3 break-all text-2xl font-semibold tabular-nums">{item.value}</p><p className="mt-2 text-xs leading-6 text-muted-foreground">{item.note}</p></Card>)}
    </div>
    <Tabs value={tab} onValueChange={value => { setTab(value as Tab); setPage(1) }} className="gap-0"><TabsList aria-label="监控内容" className="h-auto max-w-full flex-wrap"><TabsTrigger value="overview"><Activity aria-hidden="true" />运行概况</TabsTrigger><TabsTrigger value="alerts"><Bell aria-hidden="true" />异常告警{data.active_alerts > 0 && <span className="tabular-nums">{number(data.active_alerts)}</span>}</TabsTrigger><TabsTrigger value="tasks">任务队列</TabsTrigger><TabsTrigger value="audit">操作审计</TabsTrigger></TabsList>
    <div aria-busy={busy}>
    {tab === "overview" && <TabsContent value="overview"><div className="space-y-6">
      <TrafficChart data={data.timeline} />
      <div className="grid items-start gap-6 xl:grid-cols-2">
        <Panel title="服务与后台任务" note={`状态采集：${date(data.snapshot.checked_at)}；数据库和 Redis 通过真实连接检查。`}>
          <ul className="divide-y divide-border-default px-5 pb-2">{data.snapshot.components.map(component => <li key={component.name} className="flex flex-wrap items-center justify-between gap-3 py-3"><div><p className="text-sm font-medium">{{ sqlite: "业务数据库", postgres: "账户数据库", redis: "调度与限流 Redis" }[component.name] || component.name}</p><p className="mt-1 text-xs text-muted-foreground">检查耗时 {duration(component.latency_ms)}</p></div><Status value={component.healthy ? "healthy" : "unavailable"} /></li>)}{data.snapshot.workers.map(worker => <li key={worker.name} className="flex flex-wrap items-center justify-between gap-3 py-3"><div><p className="text-sm font-medium">{monitorWorkerLabels[worker.name] || worker.name}</p><p className="mt-1 text-xs text-muted-foreground">最近成功：{date(worker.last_success)}</p></div><Status value={worker.status} /></li>)}</ul>
          <div className="border-t border-border-default px-5 py-4 text-xs leading-6 text-muted-foreground"><p>本实例运行 {Math.floor(data.runtime.uptime_seconds / 60)} 分钟 · HTTP 在途 {number(data.runtime.in_flight)}</p><p>进程内存 {data.runtime.resources?.resident_memory_bytes == null ? "—" : `${(data.runtime.resources.resident_memory_bytes / 1024 / 1024).toFixed(1)} MiB`} · 打开文件 {number(data.runtime.resources?.open_fds)} · 累计 CPU {data.runtime.resources?.cpu_seconds == null ? "—" : `${data.runtime.resources.cpu_seconds.toFixed(1)}s`}</p><p>本实例记录写入失败 {number(data.runtime.log_write_failures)} 次</p></div>
        </Panel>
        <Panel title="上游调度状态" note="根据已启用账户、有效期和冷却判断候选资格；不代表实时连通性，也不含瞬时并发或模型范围校验。">
          <ScrollTable label="上游调度状态" headers={["供应商", "状态", "候选 / 全部账户"]} minWidth="min-w-[420px]">{data.snapshot.providers.length ? data.snapshot.providers.map(provider => <TableRow key={provider.id}><TableCell className={cell}><span className="font-medium">{provider.name}</span><p className="text-muted-foreground">{provider.type === "account_engine" ? "授权账户" : "API Key"}</p></TableCell><TableCell className={cell}><Status value={provider.status} /></TableCell><TableCell className={`${cell} tabular-nums`}>{number(provider.candidate_accounts)} / {number(provider.total_accounts)}</TableCell></TableRow>) : <EmptyRow columns={3}>暂无供应商状态</EmptyRow>}</ScrollTable>
        </Panel>
      </div>
      <Panel title="任务与用量" note="任务由后台持续处理；未获提交确认的任务需要核对上游记录。">
        <dl className="grid gap-5 px-5 pb-5 sm:grid-cols-3 lg:grid-cols-6">{[
          ["等待任务", data.snapshot.tasks.pending], ["提交未确认", data.snapshot.tasks.uncertain], ["等待超 15 分钟", data.snapshot.tasks.delayed], ["待结算", data.snapshot.tasks.unsettled], ["输入 Token", s.input_tokens], ["输出 Token", s.output_tokens],
        ].map(([label, value]) => <div key={String(label)}><dt className="text-xs text-muted-foreground">{label}</dt><dd className="mt-2 text-lg font-semibold tabular-nums">{number(value as number | undefined)}</dd></div>)}</dl>
      </Panel>
      <Panel title="调用最多的模型" note="当前时间范围内前 20 个模型；指标基于持久请求日志。"><ScrollTable label="模型调用表现" headers={["模型", "请求", "成功率", "限流", "输入 / 输出 Token", "已计价费用"]}>{data.models.length ? data.models.map(model => <TableRow key={model.name}><TableCell className={`${cell} max-w-64 break-words font-medium`}>{model.name}</TableCell><TableCell className={cell}>{number(model.requests)}</TableCell><TableCell className={cell}>{rate(model.success_rate)}</TableCell><TableCell className={cell}>{number(model.rate_limited)}</TableCell><TableCell className={`${cell} tabular-nums`}>{number(model.input_tokens)} / {number(model.output_tokens)}</TableCell><TableCell className={`${cell} tabular-nums`}>${model.cost_usd.toLocaleString("en-US", { maximumFractionDigits: 6 })}</TableCell></TableRow>) : <EmptyRow columns={6}>所选时间内没有调用记录</EmptyRow>}</ScrollTable></Panel>
      <p className="text-xs leading-6 text-muted-foreground">运维采集可使用后端 <code>/metrics</code> 的 Prometheus 指标；需要管理员身份或配置专用采集 Token。<Button size="sm" variant="link" render={<Link href="/console/logs" />}>查看请求日志<ExternalLink aria-hidden="true" /></Button></p>
    </div></TabsContent>}
    {tab === "alerts" && <TabsContent value="alerts"><div className="space-y-4"><div className="w-44"><label htmlFor="alert-scope" className="sr-only">告警范围</label><SelectField id="alert-scope" size="search" value={alertScope} onValueChange={value => { setAlertScope(value as "active" | "history"); setPage(1) }} options={[{value:"active",label:"当前异常"},{value:"history",label:"全部告警历史"}]} /></div><Panel title="异常告警" note="持续异常合并为同一条告警，条件不再满足时自动恢复。确认表示已查看，持续异常仍保留。"><ScrollTable label="告警列表" headers={["异常", "级别", "状态", "首次 / 最近检查", "操作"]}>{alerts?.list.length ? alerts.list.map(item => <TableRow key={item.id}><TableCell className={`${cell} max-w-96`}><p className="font-medium">{item.title}</p><p className="mt-1 text-muted-foreground">{item.detail}</p></TableCell><TableCell className={cell}><Status value={item.severity} label={item.severity === "critical" ? "严重" : "警告"} /></TableCell><TableCell className={cell}><Status value={item.resolved_at ? "resolved" : "warning"} label={item.resolved_at ? "已恢复" : item.acknowledged_at ? "持续 · 已确认" : "持续异常"} />{item.acknowledged_name && <p className="text-muted-foreground">{item.acknowledged_name} 已确认</p>}</TableCell><TableCell className={`${cell} whitespace-nowrap`}><p>{date(item.opened_at)}</p><p className="text-muted-foreground">{date(item.resolved_at || item.last_seen_at)}</p></TableCell><TableCell className={cell}><Button size="sm" variant="outline" disabled={Boolean(action || item.resolved_at || item.acknowledged_at)} onClick={() => void mutate(`alert-${item.id}`, () => monitorAPI.acknowledge(item.id), "告警已确认")}>确认告警</Button></TableCell></TableRow>) : <EmptyRow columns={5}>{busy ? "正在读取告警…" : "当前范围没有告警记录"}</EmptyRow>}</ScrollTable><Footer page={page} total={alerts?.total || 0} onPageChange={setPage} busy={busy} /></Panel></div></TabsContent>}
    {tab === "tasks" && <TabsContent value="tasks"><div className="space-y-4"><div className="w-44"><label htmlFor="task-status" className="sr-only">任务状态</label><SelectField id="task-status" size="search" value={taskStatus} onValueChange={value => { setTaskStatus(value); setPage(1) }} options={[{value:"all",label:"全部任务状态"},...["submitting","queued","processing","succeeded","failed","submit_failed"].map(value => ({value,label:monitorStatusLabels[value]}))]} /></div><Panel title="后台生成任务" note="保留最近任务记录。重新查询只安排后台读取上游结果；未获提交确认的任务需要人工核对。"><ScrollTable label="后台任务" headers={["模型 / 任务", "用户 / 密钥", "供应商", "状态", "结算", "创建 / 下次查询", "操作"]} minWidth="min-w-[920px]">{tasks?.list.length ? tasks.list.map(item => <TableRow key={item.id}><TableCell className={`${cell} max-w-64 break-all`}><p className="font-medium">{item.model}</p><code className="text-muted-foreground">{item.id}</code>{item.request_id && <p className="text-muted-foreground">请求 ID：{item.request_id}</p>}</TableCell><TableCell className={cell}><p>{item.owner_name}</p><p className="text-muted-foreground">{item.key_name}</p></TableCell><TableCell className={cell}>{item.provider_name}</TableCell><TableCell className={cell}><Status value={item.status} />{item.poll_failures > 0 && <p className="text-muted-foreground">查询失败 {number(item.poll_failures)} 次</p>}</TableCell><TableCell className={cell}>{item.settled ? "已结算" : "待结算"}</TableCell><TableCell className={`${cell} whitespace-nowrap`}><p>{date(item.created_at)}</p><p className="text-muted-foreground">{item.can_recheck ? date(item.poll_after) : "—"}</p></TableCell><TableCell className={cell}><Button size="sm" variant="outline" disabled={Boolean(action || !item.can_recheck)} onClick={() => void mutate(item.id, () => monitorAPI.recheckTask(item.id), "已安排后台重新查询")}>重新查询</Button></TableCell></TableRow>) : <EmptyRow columns={7}>{busy ? "正在读取任务…" : "当前范围没有任务"}</EmptyRow>}</ScrollTable><Footer page={page} total={tasks?.total || 0} onPageChange={setPage} busy={busy} /></Panel></div></TabsContent>}
    {tab === "audit" && <TabsContent value="audit"><Panel title="配置操作审计" note="记录操作人、动作、时间和结果；不记录请求正文、密码或密钥。历史操作从此功能启用后开始留存。"><ScrollTable label="操作审计" headers={["操作人", "操作", "结果", "时间", "请求 ID"]}>{audit?.list.length ? audit.list.map((item, index) => <TableRow key={`${item.request_id}-${index}`}><TableCell className={cell}>{item.actor_name}</TableCell><TableCell className={cell}>{auditActionLabel(item.action)}</TableCell><TableCell className={cell}><Status value={item.status_code < 400 ? "healthy" : "failed"} label={`${item.status_code} · ${item.status_code < 400 ? "成功" : "失败"}`} /></TableCell><TableCell className={`${cell} whitespace-nowrap`}>{date(item.created_at)}</TableCell><TableCell className={`${cell} font-mono`}>{item.request_id || "—"}</TableCell></TableRow>) : <EmptyRow columns={5}>{busy ? "正在读取审计…" : "暂无配置操作记录"}</EmptyRow>}</ScrollTable><Footer page={page} total={audit?.total || 0} onPageChange={setPage} busy={busy} /></Panel></TabsContent>}
    </div>
    </Tabs>
    {settingsOpen && <AlertSettings onClose={() => setSettingsOpen(false)} onSaved={() => void load()} />}
  </div>
}

function TrafficChart({ data }: { data: MonitorOverview["timeline"] }) {
  const [table, setTable] = useState(false)
  const max = Math.max(1, ...data.map(item => item.requests))
  const points = (field: "requests" | "failed") => data.map((item, index) => `${16 + index / Math.max(1, data.length - 1) * 928},${188 - item[field] / max * 164}`).join(" ")
  return <Panel title="请求趋势" note="按请求记录时间聚合；蓝线为全部请求，红线为失败，取消单列于总览。">
    <div className="space-y-3 px-5 pb-5"><div className="flex items-center justify-between text-xs text-muted-foreground"><span>每个时间段请求数 · 最高 {number(Math.max(0, ...data.map(item => item.requests)))}</span><Button size="sm" variant="ghost" onClick={() => setTable(value => !value)} aria-expanded={table}>{table ? "收起趋势数据" : "查看趋势数据"}</Button></div><svg viewBox="0 0 960 210" role="img" aria-label="所选时间范围内全部请求及失败请求趋势" className="h-40 w-full sm:h-52"><path d="M16 24 H944 M16 106 H944 M16 188 H944" fill="none" stroke="currentColor" className="text-border-default" /><polyline points={points("requests")} fill="none" stroke="currentColor" strokeWidth="2" vectorEffect="non-scaling-stroke" className="text-accent-text" /><polyline points={points("failed")} fill="none" stroke="currentColor" strokeWidth="2" strokeDasharray="4 4" vectorEffect="non-scaling-stroke" className="text-danger-text" /></svg><div className="flex justify-between gap-4 text-xs text-muted-foreground"><span>{date(data[0]?.timestamp || 0)}</span><span className="text-end">{date(data.at(-1)?.timestamp || 0)}</span></div></div>
    {table && <ScrollTable label="请求趋势数据" headers={["时间段开始", "请求数", "失败数"]} minWidth="min-w-[400px]">{data.map(item => <TableRow key={item.timestamp}><TableCell className="px-4 py-3 text-xs">{date(item.timestamp)}</TableCell><TableCell className="px-4 py-3 text-xs tabular-nums">{number(item.requests)}</TableCell><TableCell className="px-4 py-3 text-xs tabular-nums">{number(item.failed)}</TableCell></TableRow>)}</ScrollTable>}
  </Panel>
}

function AlertSettings({ onClose, onSaved }: { onClose: () => void; onSaved: () => void }) {
  const [saved, setSaved] = useState<MonitorSettings | null>(null)
  const [draft, setDraft] = useState<Record<string, string>>({})
  const [enabled, setEnabled] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const [revision, setRevision] = useState(0)
  useEffect(() => { let active = true; monitorAPI.settings().then(value => { if (active) { setSaved(value); setEnabled(value.config.enabled); setDraft(Object.fromEntries(Object.entries(value.config).filter(([key]) => key !== "enabled").map(([key,value]) => [key,String(value)]))); setError("") } }).catch(error => { if (active) setError(error instanceof Error ? error.message : "无法读取告警规则") }); return () => { active = false } }, [revision])
  const fields = [
    { key: "window_minutes", label: "统计窗口（分钟）", min: 1, max: 60 }, { key: "min_requests", label: "最小样本数", min: 1, max: 10000 },
    { key: "error_rate_percent", label: "失败率阈值（%）", min: 1, max: 100 }, { key: "rate_limit_percent", label: "429 阈值（%）", min: 1, max: 100 },
    { key: "p95_latency_ms", label: "P95 耗时阈值（ms）", min: 100, max: 600000 }, { key: "unpriced_percent", label: "未计价成功请求阈值（%）", min: 1, max: 100 },
  ]
  const save = async (event: React.FormEvent) => {
    event.preventDefault(); if (busy || !saved) return
    const invalid = fields.find(field => !draft[field.key]?.trim() || !Number.isFinite(Number(draft[field.key])) || Number(draft[field.key]) < field.min || Number(draft[field.key]) > field.max || (["window_minutes","min_requests","p95_latency_ms"].includes(field.key) && !Number.isInteger(Number(draft[field.key]))))
    if (invalid) { setError(`${invalid.label}填写 ${invalid.min}–${invalid.max}。`); document.getElementById(`rule-${invalid.key}`)?.focus(); return }
    setBusy(true); setError("")
    try { await monitorAPI.saveSettings({ enabled, ...Object.fromEntries(fields.map(field => [field.key, Number(draft[field.key])])) } as AlertConfig, saved.revision); toast.success("规则已保存，下一轮检查生效"); onSaved(); onClose() }
    catch (error) { setError(error instanceof Error ? error.message : "保存失败，请重试。") }
    finally { setBusy(false) }
  }
  return <FormDialog open onOpenChange={open => { if (!open && !busy) onClose() }} title="告警规则" description="配置调用异常的站内告警。数据库、后台任务和账户异常始终监控。" contentClassName="sm:max-w-2xl"><AppForm onSubmit={save} className="space-y-5" aria-busy={busy || !saved}><label htmlFor="alert-enabled" className="flex items-center justify-between gap-3 text-sm font-medium">启用调用异常告警<Switch id="alert-enabled" checked={enabled} onCheckedChange={setEnabled} disabled={busy || !saved} /></label><div className="grid gap-4 sm:grid-cols-2">{fields.map(field => <Field key={field.key} id={`rule-${field.key}`} label={field.label}><Input id={`rule-${field.key}`} inputMode="decimal" value={draft[field.key] || ""} onChange={event => setDraft(value => ({...value,[field.key]:event.target.value}))} disabled={busy || !saved} required /></Field>)}</div><p className="text-xs leading-6 text-muted-foreground">失败率和限流率达到最小完成样本后判断；耗时与未计价比例使用成功请求样本。持续异常保留同一条告警，恢复后再触发会创建新记录。</p><FormError message={error} />{!saved && error && <Button type="button" variant="outline" onClick={() => setRevision(value => value + 1)}>重新读取规则</Button>}<div className="flex justify-end gap-3"><Button type="button" variant="outline" disabled={busy} onClick={onClose}>取消</Button><Button type="submit" loading={busy} disabled={!saved}>保存规则</Button></div></AppForm></FormDialog>
}
