"use client"

import { ReactNode, useState } from "react"
import { ArrowDown, ChevronDown, Gauge, Layers, Minus, Plus, Repeat2 } from "lucide-react"
import { DispatchPolicy } from "@/lib/api"
import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Switch } from "@/components/ui/switch"

const strategies = [
  { value: "priority", label: "按组内顺序", description: "组内按你排列的先后顺序尝试账户。", icon: ArrowDown },
  { value: "round_robin", label: "轮流分配", description: "多份额度差不多的账户，轮流接收新请求。", icon: Repeat2 },
  { value: "weighted", label: "按比例分配", description: "额度有多有少，按你设置的份额分担。", icon: Layers },
  { value: "balanced", label: "空闲优先", description: "请求较多时，优先交给较空闲的账户。", icon: Gauge },
] as const

export function DispatchPolicyEditor({ value, onChange, disabled, accountCount, direct = false, children }: { value: DispatchPolicy; onChange: (value: DispatchPolicy) => void; disabled: boolean; accountCount: number; direct?: boolean; children?: ReactNode }) {
  const [advanced, setAdvanced] = useState(false)
  return <fieldset disabled={disabled} className="min-w-0 space-y-6 disabled:opacity-60">
    <legend className="sr-only">调用策略设置</legend>
    <div>
      <div className="flex flex-wrap items-center gap-2"><h3 className="text-base font-semibold">组内分配方式</h3><span className="rounded-md bg-bg-sunken px-2 py-1 text-xs text-muted-foreground">所有组共用</span></div>
      <div className="mt-3 grid grid-cols-2 gap-2 xl:grid-cols-4" role="group" aria-label="请求分配方式">
        {strategies.map(strategy => { const selected = value.strategy === strategy.value; return <button type="button" key={strategy.value} aria-pressed={selected} onClick={() => { if (!selected) onChange({ ...value, strategy: strategy.value, sticky_sessions: ["round_robin", "weighted"].includes(strategy.value) ? false : value.sticky_sessions }) }} className={cn("relative flex min-h-12 min-w-0 items-center gap-2 rounded-lg border px-3 py-3 text-start transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring", selected ? "border-accent-text bg-accent-subtle" : "border-border-default bg-bg-sunken hover:border-border-strong")}>
          <strategy.icon className={cn("size-4 shrink-0", selected ? "text-accent-text" : "text-muted-foreground")} aria-hidden="true" />
          <span className="min-w-0"><span className="block text-sm font-semibold">{strategy.label}</span></span>
          
        </button> })}
      </div>
      <p className="mt-2.5 text-xs leading-5 text-muted-foreground">{strategies.find(strategy => strategy.value === value.strategy)?.description} {accountCount < 2 ? "添加至少两个账户后，分配和切换策略才有实际作用。" : `当前有 ${accountCount} 个账户，调度时会跳过不可用或并发已满的账户。`}</p>
    </div>
    {children}
    
    <div className="divide-y divide-border-default overflow-hidden rounded-xl border border-border-default bg-bg-sunken">
      <div className="p-4">
        <PolicySwitch label="失败后换个账户" description={`遇到可重试错误时自动切换；每个请求最多尝试 ${value.max_attempts} 个账户。`} checked={value.max_attempts > 1} onChange={enabled => onChange({ ...value, max_attempts: enabled ? 3 : 1 })} />
      </div>
    </div>
    <button type="button" aria-expanded={advanced} onClick={() => setAdvanced(open => !open)} className="flex w-full items-center justify-between gap-3 rounded-lg px-1 py-2 text-start text-sm font-medium focus-visible:outline-2 focus-visible:outline-ring"><span>{advanced ? "收起高级设置" : "展开高级设置"}<span className="mt-1 block text-xs font-normal leading-5 text-muted-foreground">最多尝试 {value.max_attempts} 个账户 · 等待响应 {value.request_timeout_seconds} 秒 · 故障后暂停 {value.failure_cooldown_seconds} 秒 · 会话保持{value.sticky_sessions ? "开启" : "关闭"}</span></span><ChevronDown aria-hidden="true" className={cn("size-4 shrink-0 text-muted-foreground transition-transform", advanced && "rotate-180")} /></button>
    <div hidden={!advanced} className="space-y-4">
    <div className="divide-y divide-border-default rounded-xl border border-border-default bg-bg-sunken">
      <div className={value.max_attempts > 1 ? "p-4" : "hidden"}>
        {value.max_attempts > 1 && <div className="flex flex-wrap items-center justify-between gap-3">
          <div><Label htmlFor="dispatch-attempts">最多尝试账户数</Label><p id="dispatch-attempts-hint" className="mt-1 text-xs text-muted-foreground">2–100，包含第一次调用；不会重复尝试同一账户。</p></div>
          <div className="flex items-center gap-1 rounded-lg border border-border-default bg-bg-raised p-1">
            <Button type="button" variant="ghost" size="icon-sm" aria-label="减少尝试账户数" disabled={value.max_attempts <= 2} onClick={() => onChange({ ...value, max_attempts: Math.max(2, value.max_attempts - 1) })}><Minus aria-hidden="true" /></Button>
            <AttemptInput key={value.max_attempts} value={value.max_attempts} onChange={attempts => onChange({ ...value, max_attempts: attempts })} />
            <Button type="button" variant="ghost" size="icon-sm" aria-label="增加尝试账户数" disabled={value.max_attempts >= 100} onClick={() => onChange({ ...value, max_attempts: Math.min(100, value.max_attempts + 1) })}><Plus aria-hidden="true" /></Button>
          </div>
        </div>}
      </div>
      <div className="p-4"><PolicySwitch label="同一会话保持账户" description={value.strategy === "priority" ? "按组内顺序时，此选项不改变排列顺序。续接已有响应仍绑定原账户。" : direct ? "客户端提供会话标识时，在最优先的可用组内优先复用原 Key。" : "在最优先的可用组内，连续对话优先使用原账户。"} checked={value.sticky_sessions} onChange={enabled => onChange({ ...value, sticky_sessions: enabled })} />
        {["round_robin", "weighted"].includes(value.strategy) && value.sticky_sessions && <p className="mt-3 text-xs leading-5 text-accent-text">已开启会话保持：同一会话会优先复用账户，实际分配比例可能偏离轮换或权重比例。</p>}
      </div>
    </div>
    <div className="grid gap-4 rounded-xl border border-border-default bg-bg-sunken p-4 sm:grid-cols-2">
      <div className="space-y-2"><Label htmlFor="dispatch-timeout">响应期限（秒）</Label><PolicyNumber id="dispatch-timeout" value={value.request_timeout_seconds} min={5} max={600} onChange={next => onChange({ ...value, request_timeout_seconds: next })} /><p className="text-xs leading-5 text-muted-foreground">等待上游开始响应的最长时间，所有账户尝试共用。</p></div>
      <div className="space-y-2"><Label htmlFor="dispatch-cooldown">故障冷却（秒）</Label><PolicyNumber id="dispatch-cooldown" value={value.failure_cooldown_seconds} min={5} max={3600} onChange={next => onChange({ ...value, failure_cooldown_seconds: next })} /><p className="text-xs leading-5 text-muted-foreground">账户发生网络或服务故障后，暂停使用多久。上游限流时遵守其等待时间。</p></div>
    </div>
    <p className="text-xs leading-5 text-muted-foreground">{value.strategy === "round_robin" ? "选择轮流分配时会默认关闭会话保持。" : "每个账户的并发和每分钟请求上限在“账户容量”中设置。"}{direct ? "失败切换只在响应返回前发生，已开始输出后不会重放请求。" : "续接已有响应的请求仍会优先使用原账户。"}</p>
    </div>
  </fieldset>
}

function PolicySwitch({ label, description, checked, onChange }: { label: string; description: string; checked: boolean; onChange: (checked: boolean) => void }) {
  return <div className="flex items-center justify-between gap-4"><div><p className="text-sm font-medium">{label}</p><p className="mt-1 text-xs leading-5 text-muted-foreground">{description}</p></div><Switch checked={checked} onCheckedChange={onChange} aria-label={label} className="shrink-0" /></div>
}

function AttemptInput({ value, onChange }: { value: number; onChange: (value: number) => void }) {
  const [draft, setDraft] = useState(String(value))
  return <Input unstyled id="dispatch-attempts" aria-describedby="dispatch-attempts-hint" inputMode="numeric" value={draft} onChange={event => { if (/^\d{0,3}$/.test(event.target.value)) setDraft(event.target.value) }} onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); event.currentTarget.blur() } }} onBlur={() => { const next = Math.max(2, Math.min(100, Number(draft) || value)); setDraft(String(next)); onChange(next) }} className="h-9 w-14 rounded-md bg-transparent px-1 text-center tabular-nums focus-visible:outline-2 focus-visible:outline-ring" />
}

function PolicyNumber({ id, value, min, max, onChange }: { id: string; value: number; min: number; max: number; onChange: (next: number) => void }) {
  const [draft, setDraft] = useState(String(value))
  const commit = () => { const next = Math.max(min, Math.min(max, Number(draft) || value)); setDraft(String(next)); onChange(next) }
  return <Input id={id} inputMode="numeric" value={draft} onChange={event => { if (/^\d{0,4}$/.test(event.target.value)) setDraft(event.target.value) }} onBlur={commit} onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); event.currentTarget.blur() } }} />
}
