"use client"

import { Collapsible } from "@base-ui/react/collapsible"
import { ChevronDown } from "lucide-react"
import { useState, type ReactNode } from "react"
import { Button } from "@/components/coss-ui/button"

export function ModelRouteOptions({ label, summary, children, defaultOpen, open, onOpenChange }: {
  label: string; summary?: string; children: ReactNode; defaultOpen?: boolean
  open?: boolean; onOpenChange?: (open: boolean) => void
}) {
  const [initialDefaultOpen] = useState(defaultOpen)
  return <Collapsible.Root defaultOpen={initialDefaultOpen} open={open} onOpenChange={onOpenChange} className="min-w-0 border-t border-border-default pt-3">
    <Collapsible.Trigger render={<Button variant="ghost" className="group w-full justify-between whitespace-normal px-0 text-start" />}>
      <span className="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-1">
        <span>{label}</span>
        {summary && <span className="text-xs font-normal text-muted-foreground">{summary}</span>}
      </span>
      <ChevronDown className="size-4 group-data-[panel-open]:rotate-180" aria-hidden="true" />
    </Collapsible.Trigger>
    <Collapsible.Panel className="space-y-4 pt-4">{children}</Collapsible.Panel>
  </Collapsible.Root>
}
