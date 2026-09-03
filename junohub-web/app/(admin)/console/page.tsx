"use client"

import { useEffect, useMemo, useState } from "react"
import { Activity, Check, CheckCircle2, Copy, KeyRound, Network, Server } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Card, CardContent } from "@/components/ui/card"
import { cn } from "@/lib/utils"
import { consoleAPI } from "@/lib/api"
import { toast } from "sonner"
import { ErrorState, LoadingState, MetricCard, PageHead } from "./_components/console-ui"
import { useConsoleGuard } from "./_components/use-console-guard"

export default function ConsoleOverviewPage() {
  const isUser = useConsoleGuard()
  const [stats, setStats] = useState({ providers: 0, models: 0, keys: 0, requests: 0 })
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState("")

  const baseURL = useMemo(() => typeof window === "undefined" ? "/v1" : `${window.location.origin}/v1`, [])

  const load = async () => {
    setLoading(true); setLoadError("")
    try {
      const today = new Date(); today.setHours(0, 0, 0, 0)
      setStats(await consoleAPI.stats(Math.floor(today.getTime() / 1000)))
    }
    catch (error) { const message = error instanceof Error ? error.message : "无法加载网关数据，请检查网络后重试。"; setLoadError(message); toast.error(message) }
    finally { setLoading(false) }
  }

  useEffect(() => { if (isUser) load() }, [isUser])

  const copy = async (text: string, label: string) => { await navigator.clipboard.writeText(text); toast.success(`${label}已复制`) }

  if (loading) return <LoadingState />
  if (loadError) return <ErrorState message={loadError} onRetry={load} />

  const metrics = [
    { label: "供应商", value: stats.providers, note: "上游连接", icon: Server },
    { label: "模型路由", value: stats.models, note: "可调用模型", icon: Network },
    { label: "访问密钥", value: stats.keys, note: "有效凭证", icon: KeyRound },
    { label: "今日请求", value: stats.requests, note: "网关调用", icon: Activity },
  ]
  const setup = [stats.providers > 0, stats.models > 0, stats.keys > 0]

  return (
    <div className="view-enter space-y-8">
      <PageHead title="网关总览" description="集中管理上游服务、模型别名和访问密钥。所有请求通过兼容入口转发。" />

      <section aria-label="网关概览">
        <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
          {metrics.map(metric => <MetricCard key={metric.label} {...metric} />)}
        </div>
      </section>

      <section className="grid gap-4 xl:grid-cols-[minmax(0,1.4fr)_minmax(320px,.6fr)]" aria-label="快速开始">
        <Card className="gap-0 rounded-2xl py-0"><CardContent className="p-5 sm:p-6"><div className="flex flex-col gap-5 sm:flex-row sm:items-center sm:justify-between"><div><p className="text-sm font-semibold">统一请求地址</p><p className="mt-1 text-sm text-muted-foreground">在任何 OpenAI 兼容客户端中使用</p></div><span className="inline-flex w-fit items-center gap-2 rounded-full bg-success-subtle px-3 py-1.5 text-xs font-medium text-success-text"><CheckCircle2 className="size-3.5" aria-hidden="true" />服务可用</span></div><div className="mt-5 flex items-center gap-2 rounded-xl bg-bg-sunken p-2 ps-3 shadow-[inset_0_0_0_1px_var(--color-border-subtle)]"><code className="min-w-0 flex-1 break-all text-sm text-foreground"><bdi>{baseURL}</bdi></code><Button variant="outline" size="icon" onClick={() => copy(baseURL, "请求地址")} aria-label="复制统一请求地址"><Copy aria-hidden="true" /></Button></div></CardContent></Card>
        <Card className="gap-0 rounded-2xl py-0"><CardContent className="p-5 sm:p-6"><div className="flex items-center justify-between"><div><p className="text-sm font-semibold">配置进度</p><p className="mt-1 text-sm text-muted-foreground">完成三步即可发起请求</p></div><span className="metric-number text-xl">{setup.filter(Boolean).length}/3</span></div><ol className="mt-5 space-y-3">{["连接供应商", "添加模型路由", "创建访问密钥"].map((label, index) => <li key={label} className="flex items-center gap-3 text-sm"><span className={cn("grid size-6 place-items-center rounded-full", setup[index] ? "bg-success-subtle text-success-text" : "bg-bg-sunken text-muted-foreground")}>{setup[index] ? <Check className="size-3.5" aria-hidden="true" /> : <span className="text-xs tabular-nums">{index + 1}</span>}</span><span className={setup[index] ? "text-foreground" : "text-muted-foreground"}>{label}</span></li>)}</ol></CardContent></Card>
      </section>
    </div>
  )
}
