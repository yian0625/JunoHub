"use client"
import { useEffect, useState } from "react"
import { ConsoleProviderAccount } from "@/lib/api"
import { cn } from "@/lib/utils"
export function AccountHealth({ account }: { account: ConsoleProviderAccount }) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => { if (!account.cooldown_until) return; const timer = window.setInterval(() => setNow(Date.now()),1000); return () => clearInterval(timer) }, [account.cooldown_until])
  const health = account.priority === 0 ? "unassigned" : account.health || (account.status === "active" && account.schedulable ? "ready" : "disabled")
  const remaining = Math.max(0,Math.ceil((account.cooldown_until || 0) - now / 1000))
  const label = health === "unassigned" ? "未分组" : health === "ready" ? "可参与调度" : health === "expired" ? "授权已过期" : health === "disabled" ? "已停用" : `${health === "rate_limited" ? "限流" : "冷却"}${remaining ? ` · ${remaining}秒` : " · 待刷新"}`
  return <span className={cn("shrink-0 rounded-md px-2 py-1 text-xs tabular-nums",health === "ready" ? "bg-success-subtle text-success-text" : "bg-bg-raised text-muted-foreground")}>{label}</span>
}
