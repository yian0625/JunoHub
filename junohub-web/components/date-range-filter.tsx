"use client"

import { useState } from "react"
import { format, startOfMonth, subDays } from "date-fns"
import { zhCN } from "date-fns/locale"
import { CalendarIcon } from "lucide-react"
import type { DateRange } from "@daypicker/react"

import { Button } from "@/components/coss-ui/button"
import { Calendar } from "@/components/coss-ui/calendar"
import { Popover, PopoverPopup, PopoverTrigger } from "@/components/coss-ui/popover"
import { cn } from "@/lib/utils"

interface DateRangeFilterProps {
  startDate: string
  endDate: string
  onApply: (startDate: string, endDate: string) => void
  className?: string
}

export function DateRangeFilter({ startDate, endDate, onApply, className }: DateRangeFilterProps) {
  const [open, setOpen] = useState(false)
  const [draft, setDraft] = useState<DateRange | undefined>(() => toRange(startDate, endDate))
  const hasValue = Boolean(startDate || endDate)

  const choosePreset = (days: number | "month") => {
    const today = startOfDay(new Date())
    const from = days === "month" ? startOfMonth(today) : subDays(today, days - 1)
    onApply(toValue(from), toValue(today))
    setDraft({ from, to: today })
    setOpen(false)
  }

  return (
    <Popover open={open} onOpenChange={next => { setOpen(next); if (next) setDraft(toRange(startDate, endDate)) }}>
      <PopoverTrigger aria-label="日期范围" render={<Button variant="outline" className={cn("w-full justify-start font-normal", !hasValue && "text-muted-foreground", className)} />}>
        <CalendarIcon aria-hidden="true" />
        <span className="min-w-0 flex-1 truncate text-start">{rangeLabel(startDate, endDate)}</span>
      </PopoverTrigger>
      <PopoverPopup align="start" sideOffset={6} className="w-[min(22rem,calc(100vw-2rem))]">
        <div className="grid grid-cols-4 gap-1 border-b border-border-subtle pb-2">
          <Button type="button" variant={matchesPreset(draft, 1) ? "secondary" : "ghost"} size="xs" onClick={() => choosePreset(1)}>今天</Button>
          <Button type="button" variant={matchesPreset(draft, 7) ? "secondary" : "ghost"} size="xs" onClick={() => choosePreset(7)}>近 7 天</Button>
          <Button type="button" variant={matchesPreset(draft, 30) ? "secondary" : "ghost"} size="xs" onClick={() => choosePreset(30)}>近 30 天</Button>
          <Button type="button" variant={matchesPreset(draft, "month") ? "secondary" : "ghost"} size="xs" onClick={() => choosePreset("month")}>本月</Button>
        </div>
        <Calendar mode="range" locale={zhCN} selected={draft} onSelect={setDraft} resetOnSelect excludeDisabled defaultMonth={draft?.from || new Date()} disabled={{ after: new Date() }} className="mx-auto mt-2" />
        <div className="mt-2 flex items-center gap-2 border-t border-border-subtle pt-2">
          {(draft?.from || hasValue) && <Button type="button" variant="ghost" size="sm" onClick={() => { setDraft(undefined); onApply("", ""); setOpen(false) }}>清除</Button>}
          <div className="ms-auto flex gap-2">
            <Button type="button" variant="outline" size="sm" onClick={() => setOpen(false)}>取消</Button>
            <Button type="button" size="sm" disabled={!draft?.from || !draft?.to} onClick={() => { if (!draft?.from || !draft.to) return; onApply(toValue(draft.from), toValue(draft.to)); setOpen(false) }}>应用</Button>
          </div>
        </div>
      </PopoverPopup>
    </Popover>
  )
}

function toRange(startDate: string, endDate: string): DateRange | undefined {
  const from = parseValue(startDate)
  const to = parseValue(endDate)
  return from || to ? { from: from || to, to: to || from } : undefined
}

function matchesPreset(range: DateRange | undefined, days: number | "month") {
  if (!range?.from || !range.to) return false
  const today = startOfDay(new Date())
  const from = days === "month" ? startOfMonth(today) : subDays(today, days - 1)
  return toValue(range.from) === toValue(from) && toValue(range.to) === toValue(today)
}

function parseValue(value: string) {
  if (!value) return undefined
  const [year, month, day] = value.split("-").map(Number)
  return year && month && day ? new Date(year, month - 1, day) : undefined
}

function toValue(date: Date) { return format(date, "yyyy-MM-dd") }

function rangeLabel(startDate: string, endDate: string) {
  if (!startDate && !endDate) return "全部日期"
  const from = parseValue(startDate)
  const to = parseValue(endDate)
  if (!from && !to) return "全部日期"
  return `${from ? format(from, "MM/dd") : "…"} — ${to ? format(to, "MM/dd") : "…"}`
}

function startOfDay(date: Date) { return new Date(date.getFullYear(), date.getMonth(), date.getDate()) }
