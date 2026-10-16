"use client"

import { useEffect, useMemo, useState } from "react"
import { consoleAPI } from "@/lib/api"
import { toast } from "sonner"
import { ErrorState, LoadingState } from "./_components/console-ui"
import { OverviewDashboard, type OverviewStats } from "./_components/overview-dashboard"
import { useAuthStore } from "@/lib/store"
import { useConsoleGuard } from "./_components/use-console-guard"

export default function ConsoleOverviewPage() {
  const isUser = useConsoleGuard()
  const personal = useAuthStore(state => state.user?.role !== "admin")
  const [stats, setStats] = useState<OverviewStats>({ models: 0, keys: 0, requests: 0, input_tokens: 0, output_tokens: 0, total_tokens: 0 })
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState("")
  const [copied, setCopied] = useState(false)
  const baseURL = useMemo(() => typeof window === "undefined" ? "/v1" : `${window.location.origin}/v1`, [])

  const load = async () => {
    setLoading(true); setLoadError("")
    try {
      const today = new Date(); today.setHours(0, 0, 0, 0)
      setStats(await consoleAPI.stats(Math.floor(today.getTime() / 1000)))
    }
    catch (error) { const message = error instanceof Error ? error.message : "无法加载总览数据，请检查网络后重试。"; setLoadError(message); toast.error(message) }
    finally { setLoading(false) }
  }

  useEffect(() => { if (isUser) load() }, [isUser])
  useEffect(() => {
    if (!copied) return
    const timer = window.setTimeout(() => setCopied(false), 2000)
    return () => window.clearTimeout(timer)
  }, [copied])

  const copy = async () => {
    try { await navigator.clipboard.writeText(baseURL); setCopied(true); toast.success("请求地址已复制") }
    catch { toast.error("无法复制，请手动选择并复制请求地址。") }
  }

  if (loading) return <LoadingState />
  if (loadError) return <ErrorState message={loadError} onRetry={load} />

  return <OverviewDashboard stats={stats} baseURL={baseURL} copied={copied} onCopy={copy} personal={personal} />
}
