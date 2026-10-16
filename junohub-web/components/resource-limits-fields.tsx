"use client"

import { useEffect, useState } from "react"
import { consoleAPI } from "@/lib/api"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Button } from "@/components/ui/button"

export type RequestLimits = { max_concurrency: number; rpm: number }
type Kind = "provider" | "model" | "key" | "user"
type Draft = { max_concurrency: string; rpm: string }
const empty: Draft = { max_concurrency: "0", rpm: "0" }

export function useRequestLimits(kind: Kind, id: number | null, active: boolean) {
  const source = active ? `${kind}:${id ?? "new"}` : null
  const [state, setState] = useState<{ source: string | null; revision: number; saved: Draft | null; draft: Draft; error: string }>({ source: null, revision: 0, saved: null, draft: empty, error: "" })
  const [revision, setRevision] = useState(0)
  if (state.source !== source || state.revision !== revision) {
    setState({ source, revision, saved: active && id === null ? empty : null, draft: empty, error: "" })
  }
  useEffect(() => {
    let live = true
    if (!active || id === null) return
    consoleAPI.resourceLimits(kind, id).then(value => {
      if (!live) return
      const draft = { max_concurrency: String(value.max_concurrency), rpm: String(value.rpm) }
      setState(current => current.source === source && current.revision === revision ? { source, revision, saved: draft, draft, error: "" } : current)
    }).catch(error => { if (live) setState(current => current.source === source && current.revision === revision ? { source, revision, saved: null, draft: empty, error: error instanceof Error ? error.message : "请求限制加载失败" } : current) })
    return () => { live = false }
  }, [active, id, kind, source, revision])
  const ready = state.source === source && state.saved !== null
  return {
    draft: ready ? state.draft : empty,
    loading: active && !ready && !state.error,
    error: state.source === source ? state.error : "",
    dirty: ready && (state.draft.max_concurrency !== state.saved?.max_concurrency || state.draft.rpm !== state.saved?.rpm),
    change: (draft: Draft) => setState(current => ({ ...current, draft })),
    retry: () => setRevision(value => value + 1),
    value: (): RequestLimits => {
      if (!ready) throw new Error(state.error || "请求限制正在加载，请稍候再保存。")
      const max_concurrency = Number(state.draft.max_concurrency), rpm = Number(state.draft.rpm)
      if (!/^\d+$/.test(state.draft.max_concurrency) || !/^\d+$/.test(state.draft.rpm) || !Number.isSafeInteger(max_concurrency) || max_concurrency > 100000 || !Number.isSafeInteger(rpm) || rpm > 1000000) throw new Error("最大并发须为 0–100000，每分钟请求数须为 0–1000000 的整数。")
      return { max_concurrency, rpm }
    },
  }
}

export function RequestLimitsFields({ limits, prefix, disabled = false }: { limits: ReturnType<typeof useRequestLimits>; prefix: string; disabled?: boolean }) {
  return <section className="space-y-3 border-t border-border-default pt-4" aria-labelledby={`${prefix}-limits-title`} aria-busy={limits.loading}>
    <div className="flex flex-wrap items-baseline justify-between gap-2"><h3 id={`${prefix}-limits-title`} className="text-sm font-semibold">请求限制</h3><p id={`${prefix}-limits-hint`} className="text-xs text-muted-foreground">0 表示不额外限制</p></div>
    <fieldset disabled={disabled || limits.loading || Boolean(limits.error)} aria-describedby={`${prefix}-limits-hint`} className="grid min-w-0 grid-cols-1 gap-4 min-[400px]:grid-cols-2">
      <legend className="sr-only">请求限制</legend>
      {([{ field: "max_concurrency", label: "最大并发", max: 100000 }, { field: "rpm", label: "每分钟请求数（RPM）", max: 1000000 }] as const).map(({ field, label, max }) => <div key={field} className="min-w-0 space-y-2"><Label htmlFor={`${prefix}-${field}`} className="text-xs font-medium">{label}</Label><Input id={`${prefix}-${field}`} type="number" inputMode="numeric" min={0} max={max} step={1} required value={limits.draft[field]} onChange={event => limits.change({ ...limits.draft, [field]: event.target.value })} className="tabular-nums" /></div>)}
    </fieldset>
    {limits.loading && <p role="status" className="text-xs text-muted-foreground">正在读取请求限制…</p>}
    {limits.error && <div role="alert" className="flex flex-wrap items-center gap-2 text-xs text-danger-text"><p>{limits.error}</p><Button type="button" variant="ghost" size="sm" onClick={limits.retry}>重试</Button></div>}
  </section>
}
