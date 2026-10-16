"use client"

import Link from "next/link"
import { ArrowRight, Check, Copy, KeyRound, Network, Server, Terminal, Activity, Wallet } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Card, CardContent } from "@/components/ui/card"
import { Progress } from "@/components/ui/progress"
import { cn } from "@/lib/utils"
import { MetricCard, PageHead } from "./console-ui"

export type OverviewStats = {
  providers?: number
  models: number
  keys: number
  requests: number
  input_tokens: number
  output_tokens: number
  total_tokens: number
  total_requests?: number
  cost_usd?: number
  unpriced_requests?: number
}

export function OverviewDashboard({ stats, baseURL, copied, onCopy: copy, personal = false }: { stats: OverviewStats; baseURL: string; copied: boolean; onCopy: () => void; personal?: boolean }) {
  const format = (value: number) => value.toLocaleString("zh-CN")
  const metrics = personal ? [
    { label: "可用模型", value: stats.models, note: "已开放的模型", href: "/console/available-models", icon: Network },
    { label: "我的密钥", value: stats.keys, note: "个人访问密钥", href: "/console/my-keys", icon: KeyRound },
    { label: "今日请求", value: stats.requests, note: `累计 ${format(stats.total_requests || 0)} 次`, href: "/console/logs", icon: Activity },
    { label: "累计估算消耗", value: `$${(stats.cost_usd || 0).toLocaleString("en-US", {maximumFractionDigits: 6})}`, note: stats.unpriced_requests ? `${format(stats.unpriced_requests)} 次请求未计价` : "美元 · 已计价请求", href: "/console/my-keys", icon: Wallet },
  ] : [
    { label: "供应商", value: stats.providers || 0, note: "已添加连接", href: "/console/providers", icon: Server },
    { label: "模型路由", value: stats.models, note: "已配置别名", href: "/console/models", icon: Network },
    { label: "我的密钥", value: stats.keys, note: "已创建凭证", href: "/console/my-keys", icon: KeyRound },
    { label: "今日请求", value: stats.requests, note: "网关调用", href: "/console/logs", icon: Activity },
  ]
  const steps = personal ? [
    { label: "选择可用模型", href: "/console/available-models", done: stats.models > 0 },
    { label: "创建我的密钥", href: "/console/my-keys", done: stats.keys > 0 },
    { label: "按接入指引发起请求", href: "/console/guide", done: (stats.total_requests || 0) > 0 },
  ] : [
    { label: "连接供应商", href: "/console/providers", done: (stats.providers || 0) > 0 },
    { label: "添加模型路由", href: "/console/models", done: stats.models > 0 },
    { label: "创建访问密钥", href: "/console/my-keys", done: stats.keys > 0 },
  ]
  const completed = steps.filter(step => step.done).length
  const nextStep = steps.find(step => !step.done)
  const measuredTokens = stats.input_tokens + stats.output_tokens

  return (
    <div className="overview-dashboard space-y-7">
      <PageHead title={personal ? "个人总览" : "网关总览"} description={personal ? "查看你的模型调用、Token 用量和访问密钥。" : "集中管理上游服务、模型别名和访问密钥。所有请求通过兼容入口转发。"} />

      <section aria-label={personal ? "个人用量概览" : "网关概览"}>
        <div className="grid grid-cols-1 gap-4 min-[360px]:grid-cols-2 lg:grid-cols-4">
          {metrics.map(metric => <MetricCard key={metric.label} {...metric} />)}
        </div>
      </section>

      <section className="grid gap-4 xl:grid-cols-[minmax(0,1.2fr)_minmax(320px,.8fr)]" aria-label="快速开始">
        <Card role="region" aria-labelledby="endpoint-title" className="min-w-0 gap-0 py-0">
          <CardContent className="p-4 sm:p-5">
            <h2 id="endpoint-title" className="text-[15px] font-semibold">统一请求地址</h2>
            <p className="mt-1 text-[13px] leading-5 text-muted-foreground">在任何 OpenAI 兼容客户端中使用。</p>
            <div className="endpoint-code mt-4 overflow-hidden">
              <div className="endpoint-code-label flex items-center justify-between gap-3 border-b border-[var(--semantic-code-border)] px-4 py-2.5">
                <span className="inline-flex items-center gap-2 text-xs font-medium"><Terminal className="size-3.5" aria-hidden="true" />请求入口</span>
                <span className="font-mono text-[11px]">Base URL</span>
              </div>
              <div className="flex items-center gap-3 px-4 py-3">
                <code className="min-w-0 flex-1 break-words text-[13px] leading-6 sm:text-sm"><bdi>{baseURL}</bdi></code>
                <Button variant="ghost" size="sm" className="endpoint-copy shrink-0 gap-1.5" onClick={copy} aria-label={copied ? "请求地址已复制" : "复制统一请求地址"}>
                  {copied ? <Check className="size-3.5" aria-hidden="true" /> : <Copy className="size-3.5" aria-hidden="true" />}{copied ? "已复制" : "复制"}
                </Button>
              </div>
            </div>
          </CardContent>
          <div className="mt-auto grid gap-4 border-t border-border-subtle bg-bg-sunken px-4 py-3.5 sm:grid-cols-2 sm:px-5">
            <div><p className="text-xs font-medium text-muted-foreground">身份验证</p><p className="mt-1 text-[13px] leading-5">使用访问密钥作为 API Key</p></div>
            <div className="border-t border-border-subtle pt-4 sm:border-s sm:border-t-0 sm:py-0 sm:ps-4"><p className="text-xs font-medium text-muted-foreground">模型名称</p><p className="mt-1 text-[13px] leading-5">{personal ? "使用可用模型中的名称" : "填写已配置的模型别名"}</p></div>
          </div>
        </Card>

        <Card role="region" aria-labelledby="setup-title" className="min-w-0 gap-0 py-0">
          <CardContent className="p-4 sm:p-5">
            <div className="flex items-center justify-between gap-3">
              <h2 id="setup-title" className="text-[15px] font-semibold">配置进度</h2>
              <span className="rounded-full bg-bg-sunken px-2.5 py-1 text-xs font-semibold tabular-nums">{completed} / 3</span>
            </div>
            <p className="mt-1 text-[13px] leading-5 text-muted-foreground">完成三步即可发起请求。</p>
            <Progress className="mt-4" value={Math.round(completed / steps.length * 100)} aria-label="接入配置进度" aria-valuetext={"已配置 " + completed + " 项，共 " + steps.length + " 项"} />
            <ol className="mt-3 space-y-1">
              {steps.map((step, index) => (
                <li key={step.href}>
                  <Link href={step.href} className={cn("setup-link", nextStep?.href === step.href && "bg-accent-subtle ring-1 ring-inset ring-accent-border")}>
                    <span className={cn("grid size-6 shrink-0 place-items-center rounded-full text-xs tabular-nums", step.done ? "bg-success-subtle text-success-text" : "border border-border-default text-muted-foreground")}>{step.done ? <Check className="size-3" aria-hidden="true" /> : index + 1}</span>
                    <span className="min-w-0 flex-1"><span className="block text-[13px] leading-[18px] font-medium">{step.label}</span><span className="mt-0.5 block text-xs leading-4 text-muted-foreground">{step.done ? personal ? "已就绪" : "已配置" : nextStep?.href === step.href ? "下一步" : "待完成"}</span></span>
                    <ArrowRight className={cn("size-3.5 shrink-0", nextStep?.href === step.href ? "text-accent-text" : "text-muted-foreground")} aria-hidden="true" />
                  </Link>
                </li>
              ))}
            </ol>
          </CardContent>
        </Card>
      </section>

      <section aria-labelledby="tokens-title">
        <Card className="gap-0 py-0">
          <CardContent className="p-4 sm:p-5">
            <div className="flex flex-wrap items-end justify-between gap-4">
              <div><h2 id="tokens-title" className="text-[15px] font-semibold">今日 Token 用量</h2><p className="mt-1 text-[13px] text-muted-foreground">本地时间 00:00 起 · 截至本次加载</p></div>
              <div className="sm:text-end"><p className="text-xs font-medium text-muted-foreground">合计</p><p className="mt-1 font-mono text-2xl leading-none font-medium tracking-[-.055em] tabular-nums">{format(stats.total_tokens)}<span className="ms-2 font-sans text-xs tracking-normal text-muted-foreground">Token</span></p></div>
            </div>
            <div className="token-track mt-5" aria-hidden="true">
              {measuredTokens > 0 && <>{stats.input_tokens > 0 && <span className="token-input" style={{ flexGrow: stats.input_tokens, flexBasis: 0 }} />}{stats.output_tokens > 0 && <span className="token-output" style={{ flexGrow: stats.output_tokens, flexBasis: 0 }} />}</>}
            </div>
            <div className="mt-4 flex flex-wrap gap-x-8 gap-y-2 border-t border-border-subtle pt-3.5 text-[13px] text-muted-foreground">
              <span className="inline-flex items-center gap-2"><i className="token-input size-2 rounded-full" aria-hidden="true" />输入 <span className="font-mono tabular-nums text-foreground">{format(stats.input_tokens)}</span></span>
              <span className="inline-flex items-center gap-2"><i className="token-output size-2 rounded-full" aria-hidden="true" />输出 <span className="font-mono tabular-nums text-foreground">{format(stats.output_tokens)}</span></span>
            </div>
          </CardContent>
        </Card>
      </section>
    </div>
  )
}
