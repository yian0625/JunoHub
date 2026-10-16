export interface MonitorSummary {
  requests: number; success: number; failed: number; pending: number; cancelled: number;
  success_rate: number | null; p50_ms: number | null; p95_ms: number | null; average_ms: number | null;
  rate_limited: number; server_errors: number; unpriced: number; cost_usd: number;
  input_tokens: number; output_tokens: number;
}
export interface MonitorOverview {
  summary: MonitorSummary;
  timeline: Array<{ timestamp: number; requests: number; failed: number }>;
  models: Array<Omit<MonitorSummary, "p50_ms" | "p95_ms" | "average_ms"> & { name: string }>;
  snapshot: {
    checked_at: number; stale: boolean;
    components: Array<{ name: string; healthy: boolean; latency_ms: number }>;
    workers: Array<{ name: string; status: string; last_tick: number; last_success: number }>;
    providers: Array<{ id: number; name: string; type: string; status: string; total_accounts: number | null; candidate_accounts: number | null; cooling_accounts: number | null }>;
    tasks: Partial<{ retained: number; pending: number; uncertain: number; delayed: number; unsettled: number; overdue: number }>;
  };
  runtime: { resources?: { resident_memory_bytes: number | null; open_fds: number | null; cpu_seconds: number | null }; uptime_seconds: number; in_flight: number; http_requests: number; log_write_failures: number };
  active_alerts: number; generated_at: number; from: number; to: number; hours: number;
}
export interface MonitorAlert {
  id: number; rule: string; severity: string; title: string; detail: string;
  opened_at: number; last_seen_at: number; resolved_at: number; acknowledged_at: number; acknowledged_name: string; occurrences: number;
}
export interface MonitorTask {
  id: string; model: string; status: string; created_at: number; expires_at: number; poll_after: number;
  poll_failures: number; settled: boolean; request_id: string; can_recheck: boolean;
  key_name: string; provider_name: string; owner_name: string;
}
export interface MonitorAudit { actor_name: string; action: string; request_id: string; status_code: number; created_at: number }
export interface AlertConfig { enabled: boolean; window_minutes: number; min_requests: number; error_rate_percent: number; rate_limit_percent: number; p95_latency_ms: number; unpriced_percent: number }
export interface MonitorSettings { config: AlertConfig; revision: number; updated_at: number }
export interface MonitorList<T> { list: T[]; total: number; page: number }
export const monitorStatusLabels: Record<string, string> = { healthy: "正常", unavailable: "异常", starting: "启动中", disabled: "已停用", unknown: "未获确认", eligible: "可参与调度", no_accounts: "未配置账户", submitting: "等待提交确认", queued: "排队中", processing: "处理中", succeeded: "已完成", failed: "失败", submit_failed: "提交失败" }
export const monitorWorkerLabels: Record<string, string> = { monitor: "监控采集", task_poller: "任务查询与结算", account_scheduler: "账户调度同步" }
export function auditActionLabel(action: string) {
  const resources: Record<string,string> = { provider: "供应商", model: "模型路由", key: "访问密钥", users: "用户", guide: "接入指引", integration: "客户端配置", monitor: "监控" }
  const operations: Record<string,string> = { create: "创建", update: "修改", delete: "删除", import: "导入", upsert: "保存", sync: "同步", reorder: "排序", dispatch: "配置调度", complete: "完成授权", acknowledge: "确认告警", recheck: "查询任务" }
  if (action.endsWith("/acknowledge")) return "确认告警"
  if (action.endsWith("/recheck")) return "安排任务查询"
  const parts = action.split("/").filter(Boolean)
  return `${operations[parts.at(-1) || ""] || "配置"}${resources[parts[1]] || "设置"}`
}
