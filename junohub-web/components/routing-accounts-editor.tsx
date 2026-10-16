"use client"

import { useEffect, useState } from "react"
import { ArrowUp, ArrowDown, Check, ChevronDown, KeyRound, Plus, SearchIcon, Users, X } from "lucide-react"
import { ConsoleProviderAccount } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/coss-ui/input-group"
import { cn } from "@/lib/utils"

export type RoutingDraft = Record<number, { priority: number; member_order: number; weight: number }>

export function RoutingAccountsEditor({ accounts, strategy, onChange, onInvalidChange, maxAttempts }: { accounts: ConsoleProviderAccount[]; strategy: string; onChange: (draft: RoutingDraft) => void; onInvalidChange: (invalid: boolean) => void; maxAttempts: number }) {
  const [groupIds, setGroupIds] = useState(() => [...new Set(accounts.filter(a => a.priority > 0).map(a => a.priority))].sort((a, b) => a - b))
  const groups = [...new Set([...groupIds, ...accounts.filter(a => a.priority > 0).map(a => a.priority)])].sort((a, b) => a - b)
  const [adding, setAdding] = useState(false)
  const [query, setQuery] = useState("")
  const [showRules, setShowRules] = useState(false)
  const [selected, setSelected] = useState<number | null>(null)
  const current = selected !== null && groups.includes(selected) ? selected : groups[0]
  const index = groups.indexOf(current)
  const name = (tier: number) => groups.indexOf(tier) === 0 ? "主用组" : `备用组 ${groups.indexOf(tier)}`
  const members = accounts.filter(a => a.priority === current).sort((a, b) => (a.member_order ?? 0) - (b.member_order ?? 0) || a.id - b.id)
  const available = accounts.filter(a => a.priority === 0).sort((a, b) => (a.member_order ?? 0) - (b.member_order ?? 0) || a.id - b.id)
  const filtered = available.filter(account => account.name.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()))
  const empty = groups.filter(tier => !accounts.some(a => a.priority === tier))
  const invalid = empty.length > 0
  useEffect(() => { onInvalidChange(invalid); return () => onInvalidChange(false) }, [invalid, onInvalidChange])
  const update = (rows: ConsoleProviderAccount[]) => onChange(Object.fromEntries(rows.map(a => [a.id, { priority: a.priority, member_order: a.member_order ?? 0, weight: a.weight ?? 1 }])))
  const move = (id: number, target: number) => {
    const targetMembers = accounts.filter(a => a.priority === target && a.id !== id).sort((a, b) => (a.member_order ?? 0) - (b.member_order ?? 0) || a.id - b.id)
    update(accounts.map(a => a.id === id ? { ...a, priority: target, member_order: targetMembers.length + 1 } : targetMembers.some(m => m.id === a.id) ? { ...a, member_order: targetMembers.findIndex(m => m.id === a.id) + 1 } : a))
  }
  const moveMember = (index: number, direction: number) => {
    const next = [...members]; [next[index], next[index + direction]] = [next[index + direction], next[index]]
    update(accounts.map(a => a.priority === current ? { ...a, member_order: next.findIndex(m => m.id === a.id) + 1 } : a))
  }
  const addGroup = () => {
    if (groups.length >= 999) return
    if (Math.max(...groups, 0) >= 999) {
      const compact = groups.map((_, i) => i + 1)
      update(accounts.map(a => ({ ...a, priority: groups.indexOf(a.priority) + 1 })))
      setGroupIds([...compact, compact.length + 1]); setSelected(compact.length + 1)
    } else {
      const next = Math.max(...groups, 0) + 1
      setGroupIds([...groups, next]); setSelected(next)
    }
  }
  const reorder = (direction: number) => {
    const other = groups[index + direction]
    update(accounts.map(a => ({ ...a, priority: a.priority === current ? other : a.priority === other ? current : a.priority })))
    setSelected(other)
  }
  const total = members.reduce((sum, a) => sum + (a.weight ?? 1), 0)
  return <section className="space-y-4" aria-label="分组编辑器">
    <div className="flex flex-wrap items-center justify-between gap-3"><div><h3 className="text-base font-semibold">调用分组</h3><p className="mt-1 text-xs leading-6 text-muted-foreground">先使用主用组，再依次尝试备用组。</p></div><Button type="button" variant="outline" size="sm" disabled={!accounts.length || groups.length >= 999} onClick={addGroup}><Plus aria-hidden="true" />新建分组</Button></div>
    {!accounts.length ? <p className="py-6 text-sm text-muted-foreground">先添加账户，再创建分组。</p> : <div className="grid overflow-hidden rounded-xl border border-border-default lg:grid-cols-[12rem_minmax(0,1fr)]">
      <nav aria-label="选择分组" className="min-w-0 border-b border-border-default bg-bg-sunken p-2 lg:border-b-0 lg:border-e lg:p-3">
        <p className="px-2 py-2 text-xs text-muted-foreground">分组优先级</p>
        <div className="flex gap-2 overflow-x-auto pb-1 lg:flex-col">{groups.map((tier, i) => <button type="button" key={tier} aria-pressed={current === tier} onClick={() => { setSelected(tier); setAdding(false); setQuery("") }} className={cn("flex min-h-16 shrink-0 items-center gap-3 rounded-lg border px-3 py-2.5 text-start focus-visible:outline-2 focus-visible:outline-ring lg:w-full", current === tier ? "border-border-strong bg-bg-surface shadow-sm" : "border-transparent hover:bg-bg-surface")}>
          <span className={cn("grid size-7 shrink-0 place-items-center rounded-md text-xs font-semibold tabular-nums", current === tier ? "bg-accent-subtle text-accent-text" : "text-muted-foreground")}>{i + 1}</span>
          <span className="flex-1"><span className="block text-sm font-semibold">{name(tier)}</span><span className="mt-1 block text-xs text-muted-foreground">{accounts.filter(a => a.priority === tier).length} 个账户</span></span>
          {current === tier && <Check className="size-3.5 shrink-0 text-accent-text" aria-hidden="true" />}
        </button>)}</div>
      </nav>
      {groups.length > 0 ? <div className="min-w-0 space-y-5 bg-bg-surface p-4 sm:p-5" aria-label={`编辑${name(current)}`} role="region">
        <div className="flex flex-wrap items-start justify-between gap-3"><div><h4 className="text-base font-semibold">{name(current)}</h4><p className="mt-1 text-xs leading-6 text-muted-foreground">第 {index + 1} 个使用{index === 0 ? "，优先接收请求。" : "，前面的组无法完成时按策略接替。"}</p></div><div className="flex gap-1"><Button type="button" variant="outline" size="sm" disabled={index <= 0} onClick={() => reorder(-1)}><ArrowUp aria-hidden="true" />提前</Button><Button type="button" variant="outline" size="sm" disabled={index >= groups.length - 1} onClick={() => reorder(1)}><ArrowDown aria-hidden="true" />延后</Button></div></div>
        <section aria-label="本组账户" className="space-y-3"><p className="text-xs leading-6 text-muted-foreground">{strategy === "priority" ? "按下面顺序逐个尝试，可用箭头调整。" : strategy === "round_robin" ? "按下面顺序轮流接收新请求，可用箭头调整轮换顺序。" : strategy === "weighted" ? "按份额选择账户，列表位置不决定谁先被选中。" : "优先选择较空闲的账户，列表位置不决定谁先被选中。"}</p><div className="flex items-center gap-2"><Users className="size-4 text-muted-foreground" aria-hidden="true" /><h5 className="text-sm font-semibold">本组账户</h5><span className="text-xs text-muted-foreground">{members.length} 个</span><Button type="button" variant="outline" size="sm" className="ms-auto" aria-expanded={adding} onClick={() => { setAdding(open => !open); setQuery("") }}><Plus aria-hidden="true" />加入账户</Button></div>
        {adding && <section className="space-y-3 rounded-lg border border-border-strong bg-bg-sunken p-3" aria-label="可加入的账户"><div className="flex items-center justify-between gap-2"><h5 className="text-sm font-semibold">选择要加入的账户</h5><Button type="button" variant="ghost" size="icon-sm" aria-label="关闭账户选择" onClick={() => setAdding(false)}><X aria-hidden="true" /></Button></div><InputGroup><InputGroupInput type="search" aria-label="搜索未分组账户" placeholder="搜索账户名称" value={query} onChange={event => setQuery(event.target.value)} /><InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon></InputGroup>{available.length ? <div className="max-h-80 divide-y divide-border-default overflow-y-auto">{filtered.map(account => <div key={account.id} className="flex items-center justify-between gap-3 py-3"><div className="min-w-0"><p className="break-words text-sm [overflow-wrap:anywhere]">{account.name}</p><p className="mt-1 text-xs text-muted-foreground">未分组 · 不参与调用</p></div><Button type="button" variant="outline" size="sm" aria-label={`加入 ${account.name}`} onClick={() => move(account.id, current)}>加入本组</Button></div>)}{!filtered.length && <p className="py-3 text-xs text-muted-foreground">没有匹配的账户，请换个名称搜索。</p>}</div> : <p className="text-xs leading-6 text-muted-foreground">没有未分组账户。可从其他组移除账户，或先添加新的授权账户。</p>}</section>}
        {members.length ? <div className="divide-y divide-border-default">{members.map((account, memberIndex) => <div key={account.id} className="flex flex-wrap items-center gap-x-4 gap-y-2 py-3"><div className="flex min-w-0 basis-full items-center gap-3 sm:flex-1 sm:basis-auto"><span className="grid size-7 shrink-0 place-items-center rounded-md bg-bg-sunken text-xs tabular-nums text-muted-foreground" aria-hidden="true">{["priority", "round_robin"].includes(strategy) ? memberIndex + 1 : <KeyRound className="size-3.5" />}</span><p className="min-w-0 break-words text-sm font-medium [overflow-wrap:anywhere]">{account.name}</p></div><div className="ms-auto flex flex-wrap items-center gap-2">{strategy === "weighted" && <div><label className="text-xs text-muted-foreground" htmlFor={`group-weight-${account.id}`}>分配份额</label><div className="mt-1 flex items-center gap-2"><Input id={`group-weight-${account.id}`} aria-label={`${account.name} 分配份额`} className="h-11 w-20 px-2 text-center" inputMode="numeric" value={account.weight || ""} onChange={event => { if (/^\d{0,4}$/.test(event.target.value)) update(accounts.map(a => a.id === account.id ? { ...a, weight: Number(event.target.value) } : a)) }} /><span className="text-xs tabular-nums text-accent-text">本组约 {total ? Math.round((account.weight ?? 1) / total * 100) : 0}%</span></div></div>}{["priority", "round_robin"].includes(strategy) && <div className="flex gap-1"><Button type="button" variant="outline" size="icon-sm" aria-label={`提前调用 ${account.name}`} disabled={memberIndex === 0} onClick={() => moveMember(memberIndex, -1)}><ArrowUp aria-hidden="true" /></Button><Button type="button" variant="outline" size="icon-sm" aria-label={`延后调用 ${account.name}`} disabled={memberIndex === members.length - 1} onClick={() => moveMember(memberIndex, 1)}><ArrowDown aria-hidden="true" /></Button></div>}<Button type="button" variant="ghost" size="sm" className="text-muted-foreground hover:text-danger-text" aria-label={`移除 ${account.name}`} onClick={() => move(account.id, 0)}>移除</Button></div></div>)}</div> : <div className="space-y-3 rounded-lg bg-bg-sunken p-4"><p className="text-sm leading-6">这个组还没有账户。点击“加入账户”选择成员。</p><Button type="button" variant="outline" size="sm" onClick={() => { setGroupIds(groups.filter(tier => tier !== current)); setSelected(null) }}>移除空分组</Button></div>}
        </section>

      </div> : <div className="p-5 text-sm text-muted-foreground">当前没有分组。点击“新建分组”后加入账户。</div>}
    </div>}
    {invalid && <p role="status" className="rounded-lg bg-accent-subtle p-3 text-xs leading-6 text-accent-text">{empty.map(name).join("、")}尚无账户。请加入账户或移除空组后保存。</p>}
    <div className="rounded-lg bg-bg-sunken px-3"><button type="button" aria-expanded={showRules} onClick={() => setShowRules(open => !open)} className="flex min-h-11 w-full items-center justify-between gap-3 text-start text-xs text-muted-foreground focus-visible:outline-2 focus-visible:outline-ring"><span>调用规则 · 每个请求最多尝试 {maxAttempts} 个账户</span><ChevronDown aria-hidden="true" className={cn("size-4 shrink-0 transition-transform", showRules && "rotate-180")} /></button><p hidden={!showRules} className="pb-3 text-xs leading-6 text-muted-foreground">{strategy === "weighted" ? "份额只在同组内计算，例如 7 和 3 约分担 70% 和 30%。" : strategy === "round_robin" ? "组内轮流使用账户。" : strategy === "balanced" ? "组内优先使用较空闲的账户。" : "组内按设置的顺序尝试。"}从分组移除只解除分配，不删除账户；未分组账户不参与调用。组间按优先级调用，跳过不可用、达到限额或不支持模型的账户。每个请求合计最多尝试 {maxAttempts} 个账户（含首次），达到上限后不会继续到下一组。续接已有响应仍需使用原账户。</p></div>
  </section>
}
