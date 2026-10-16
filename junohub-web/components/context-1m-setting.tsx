"use client"

import { Label } from "@/components/ui/label"
import { Switch } from "@/components/ui/switch"

export function Context1MSetting({ id, checked, onCheckedChange, disabled }: {
  id: string; checked: boolean; onCheckedChange: (checked: boolean) => void; disabled?: boolean
}) {
  return <div className="flex min-w-0 items-center justify-between gap-4 rounded-lg border border-border-default px-3.5 py-3">
    <div className="min-w-0 space-y-1">
      <Label htmlFor={id}>1M 上下文兼容标记</Label>
      <p id={`${id}-description`} className="text-xs leading-5 text-muted-foreground">仅在供应商要求时启用</p>
    </div>
    <Switch id={id} size="sm" checked={checked} onCheckedChange={onCheckedChange} disabled={disabled} aria-describedby={`${id}-description`} />
  </div>
}
