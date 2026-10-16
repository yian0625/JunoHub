"use client"

import { useEffect, useId, useState } from "react"
import { Combobox } from "@base-ui/react/combobox"
import { Check, ChevronDown, RefreshCw, SearchIcon } from "lucide-react"
import { matchesModel } from "@/lib/model-search"
import { consoleAPI } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/coss-ui/input-group"

export function UpstreamModelPicker({ providerId, value, onChange, purpose = "route", disabled = false, compact = false, id }: { providerId: number; value: string; onChange: (value: string) => void; purpose?: "route" | "test"; disabled?: boolean; compact?: boolean; id?: string }) {
  const generatedId = useId()
  const inputId = id || generatedId
  const [manual, setManual] = useState(false)
  const [revision, setRevision] = useState(0)
  const requestKey = `${providerId}:${revision}`
  const [result, setResult] = useState<{ key: string; models: string[]; error: string } | null>(null)
  const loading = result?.key !== requestKey
  const models = loading ? [] : result.models
  const error = loading ? "" : result.error

  useEffect(() => {
    let active = true
    consoleAPI.providerModels(providerId).then(data => {
      if (active) setResult({ key: requestKey, models: [...new Set(data.models || [])].sort(), error: "" })
    }).catch(error => {
      if (active) setResult({ key: requestKey, models: [], error: error instanceof Error ? error.message : "无法获取模型列表。" })
    })
    return () => { active = false }
  }, [providerId, requestKey])

  return <fieldset disabled={disabled} className={compact ? "min-w-0 space-y-2" : "space-y-3 rounded-xl border border-border-default bg-bg-sunken p-4"} aria-label={purpose === "test" ? "选择测试模型" : "上游模型"}>
    <div className="flex flex-wrap items-center justify-between gap-2">
      <Label htmlFor={inputId}>{purpose === "test" ? "测试模型" : "上游模型名称"}</Label>
      <Button type="button" variant="ghost" size="sm" disabled={disabled || loading} onClick={() => setRevision(value => value + 1)}><RefreshCw className={loading ? "animate-spin motion-reduce:animate-none" : undefined} aria-hidden="true" />{loading ? "获取中" : compact ? "刷新" : models.length ? "刷新模型" : "获取模型"}</Button>
    </div>
    {manual ? <Input id={inputId} name="upstream-model" value={value} onChange={event => onChange(event.target.value)} placeholder="填写完整模型 ID" spellCheck={false} required={purpose === "route"} /> : <>
      <Combobox.Root key={providerId} items={models} value={value || null} onValueChange={next => onChange(next ?? "")} disabled={disabled || loading} openOnInputClick autoHighlight filter={matchesModel}>
        <Combobox.InputGroup render={<InputGroup />}>
          <Combobox.Input id={inputId} name="upstream-model-search" placeholder={loading ? "正在获取供应商模型…" : "输入关键词搜索模型"} autoComplete="off" spellCheck={false} render={<InputGroupInput nativeInput />} />
          <InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon>
          <InputGroupAddon align="inline-end"><Combobox.Trigger aria-label="展开模型列表" className="grid size-6 place-items-center rounded-md text-muted-foreground focus-visible:outline-2 focus-visible:outline-ring"><ChevronDown className="size-4" aria-hidden="true" /></Combobox.Trigger></InputGroupAddon>
        </Combobox.InputGroup>
        <Combobox.Portal>
          <Combobox.Positioner sideOffset={6} align="start" collisionPadding={12} className="z-[60] max-w-[calc(100vw-1.5rem)]">
            <Combobox.Popup className="choice-popup w-[var(--anchor-width)] max-w-[calc(100vw-1.5rem)] p-1.5">
              <Combobox.Empty className="px-3 py-3 text-sm text-muted-foreground empty:hidden">没有匹配的模型，请修改关键词或手动填写模型 ID。</Combobox.Empty>
              <Combobox.List className="max-h-[min(20rem,var(--available-height))] overflow-y-auto overscroll-contain">
                {(model: string) => <Combobox.Item key={model} value={model} className="choice-item relative flex min-h-10 items-center rounded-lg py-2 ps-3 pe-9 text-sm outline-none [overflow-wrap:anywhere]">
                  {model}<Combobox.ItemIndicator className="absolute end-3 text-accent-text"><Check className="size-4" aria-hidden="true" /></Combobox.ItemIndicator>
                </Combobox.Item>}
              </Combobox.List>
            </Combobox.Popup>
          </Combobox.Positioner>
        </Combobox.Portal>
      </Combobox.Root>
      {!compact && value && <p className="text-xs leading-5 text-muted-foreground [overflow-wrap:anywhere]">当前选择：{value}</p>}
    </>}
    {(!compact || error || (!loading && !models.length)) && <p role={error ? "alert" : "status"} className={`text-xs leading-5 [overflow-wrap:anywhere] ${error ? "text-danger-text" : "text-muted-foreground"}`}>
      {error ? `${error} 可重试获取，或手动填写模型 ID。` : (loading ? "正在向供应商获取模型列表。" : models.length ? `已获取 ${models.length} 个模型，${purpose === "test" ? "选择后可验证实际调用" : "选择后可设置对外别名"}。` : "供应商未返回模型，可重试或手动填写。")}
    </p>}
    <Button type="button" variant="link" size="sm" disabled={disabled} className="h-auto px-0 py-1 text-accent-text" onClick={() => setManual(!manual)}>{manual ? "返回模型列表" : "手动填写模型 ID"}</Button>
  </fieldset>
}
