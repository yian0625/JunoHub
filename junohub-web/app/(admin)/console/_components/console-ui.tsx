"use client"

import Link from "next/link"
import { ArrowUpRight, Plus, RotateCw, SearchIcon, Trash2, Zap } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Card, CardContent, CardFooter } from "@/components/ui/card"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty"
import { Field as FieldPrimitive, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/coss-ui/input-group"
import { Spinner } from "@/components/ui/spinner"
import { cn } from "@/lib/utils"

export function PageHead({ title, description, action }: { title: string; description: string; action?: React.ReactNode }) {
  return (
    <header className="flex flex-col gap-5 sm:flex-row sm:items-start sm:justify-between">
      <div className="min-w-0">
        <h1 className="page-heading">{title}</h1>
        <p className="page-lead mt-2.5">{description}</p>
      </div>
      {action && <div className="shrink-0">{action}</div>}
    </header>
  )
}

export function MetricCard({ label, value, note, href, icon: Icon }: { label: string; value: number | string; note: string; href: string; icon: React.ElementType }) {
  const formatted = typeof value === "number" ? value.toLocaleString("zh-CN") : value
  return <Link href={href} className="group block min-w-0 rounded-2xl focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
    <Card className="h-full min-w-0 gap-0 py-0 [container-type:inline-size] transition-[border-color,box-shadow] duration-150 group-hover:border-border-strong group-hover:shadow-[var(--shadow-surface-hover)]">
      <CardContent className="flex min-h-32 flex-col p-4">
        <div className="flex items-start justify-between gap-3"><p className="pt-0.5 text-[13px] font-medium text-muted-foreground">{label}</p><span className="grid size-7 shrink-0 place-items-center rounded-lg border border-border-subtle bg-bg-sunken text-foreground"><Icon className="size-3.5" strokeWidth={1.75} aria-hidden="true" /></span></div>
        <div className="mt-auto pt-3"><p className={cn("break-all font-mono leading-none font-medium tracking-[-.055em] tabular-nums", formatted.length > 9 ? "text-[clamp(1rem,10cqi,1.625rem)]" : "text-[26px]")}>{formatted}</p><div className="mt-2 flex items-center justify-between gap-2"><p className="text-xs text-muted-foreground">{note}</p><ArrowUpRight className="size-3 shrink-0 text-muted-foreground group-hover:text-accent-text" aria-hidden="true" /></div></div>
      </CardContent>
    </Card>
  </Link>
}

export function ResourceGrid({ children }: { children: React.ReactNode }) {
  return <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">{children}</div>
}

export function ResourceToolbar({ query, onQueryChange, count, total, label }: { query: string; onQueryChange: (value: string) => void; count: number; total: number; label: string }) {
  return (
    <div className="flex flex-col gap-3 border-b border-border-default pb-4 sm:flex-row sm:items-center sm:justify-between">
      <p className="text-sm font-medium text-foreground tabular-nums" role="status">{query.trim() ? `找到 ${count} / ${total} 个${label}` : `共 ${total} 个${label}`}</p>
      <InputGroup className="sm:max-w-80">
        <InputGroupInput type="search" aria-label={`搜索${label}`} placeholder={`搜索${label}`} value={query} onChange={event => onQueryChange(event.target.value)} />
        <InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon>
      </InputGroup>
    </div>
  )
}

export function NoSearchResults({ onClear }: { onClear: () => void }) {
  return <Card className="py-0"><Empty className="py-12"><EmptyHeader><EmptyTitle>没有匹配的结果</EmptyTitle><EmptyDescription>尝试其他名称，或清除搜索查看全部资源。</EmptyDescription></EmptyHeader><EmptyContent><Button variant="outline" onClick={onClear}>清除搜索</Button></EmptyContent></Empty></Card>
}

export function ResourceCard({ badge, title, meta, metaLabel = "连接信息", status, actions, onDelete, deleteLabel, children, compact = false, headingLevel = 2 }: { headingLevel?: 2 | 3; badge: React.ReactNode; title: string; meta?: string; metaLabel?: string; children?: React.ReactNode; compact?: boolean; status: { active: boolean; label: string }; actions?: React.ReactNode; onDelete?: () => void; deleteLabel?: string }) {
  const Title = headingLevel === 3 ? "h3" : "h2"
  return (
    <Card className={cn("resource-card min-w-0 gap-0 py-0", !compact && "min-h-52")}>
      <CardContent className={cn("flex-1", compact ? "p-4" : "p-5")}>
        <div className="flex items-start gap-3">
          <span className={cn("grid shrink-0 place-items-center overflow-hidden rounded-xl border border-border-default bg-bg-sunken text-foreground", compact ? "size-8" : "size-10")}>{badge}</span>
          <div className="min-w-0 flex-1">
            <Title className="text-base font-semibold leading-6 [overflow-wrap:anywhere]">{title}</Title>
            <Badge variant="secondary" className={cn("mt-1.5 gap-1.5", status.active ? "bg-success-subtle text-success-text" : "bg-bg-sunken text-muted-foreground")}>
              <span className={cn("size-1.5 rounded-full", status.active ? "bg-success-solid" : "bg-text-disabled")} aria-hidden="true" />{status.label}
            </Badge>
          </div>
        </div>
        {children ? <div className={compact ? "mt-3" : "mt-5"}>{children}</div> : <div className="mt-5 border-t border-border-subtle pt-4"><p className="mb-1 text-xs text-muted-foreground">{metaLabel}</p><p className="break-all font-mono text-xs leading-6 text-foreground">{meta}</p></div>}
      </CardContent>
      <CardFooter className={cn("resource-card-footer flex flex-wrap items-center justify-between gap-2", compact ? "px-3 py-2" : "min-h-16 px-4 py-3")}>
        {onDelete && <Button size="sm" variant="ghost" onClick={onDelete} className="text-muted-foreground hover:bg-danger-subtle hover:text-danger-text" aria-label={deleteLabel} title={deleteLabel}><Trash2 aria-hidden="true" />删除</Button>}
        {actions}
      </CardFooter>
    </Card>
  )
}

export function AddCard({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} className="group flex min-h-52 flex-col items-center justify-center gap-3 rounded-xl border border-dashed border-border-default bg-transparent p-5 text-muted-foreground transition-colors duration-150 hover:border-accent-solid/40 hover:bg-[var(--glass-bg-subtle)] hover:text-accent-text focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
      <span className="grid size-10 place-items-center rounded-full ring-1 ring-inset ring-border-default transition-colors duration-150 group-hover:ring-accent-solid/40"><Plus className="size-5" strokeWidth={1.75} aria-hidden="true" /></span>
      <span className="text-sm font-medium">{label}</span>
    </button>
  )
}

export function FormDialog({ open, onOpenChange, title, description, children, contentClassName, footer }: { open: boolean; onOpenChange: (open: boolean) => void; title: string; description: string; children: React.ReactNode; contentClassName?: string; footer?: React.ReactNode }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={cn("glass-popup grid grid-rows-[auto_minmax(0,1fr)] gap-0 overflow-clip rounded-2xl p-0 sm:max-w-md", footer && "grid-rows-[auto_minmax(0,1fr)_auto]", contentClassName)}>
        <DialogHeader className="border-b border-[var(--glass-border-subtle)] bg-bg-page px-6 pb-5 pt-6">
          <DialogTitle className="pe-8 text-base font-semibold">{title}</DialogTitle>
          <DialogDescription className="max-w-2xl text-sm leading-6 text-pretty">{description}</DialogDescription>
        </DialogHeader>
        <div className="min-h-0 overflow-y-auto overscroll-contain px-6 py-5">{children}</div>
        {footer && <div className="border-t border-border-default bg-bg-page px-6 py-4">{footer}</div>}
      </DialogContent>
    </Dialog>
  )
}

export function Field({ id, label, hint, children }: { id: string; label: string; hint?: string; children: React.ReactNode }) {
  return <FieldPrimitive><div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between sm:gap-3"><FieldLabel htmlFor={id}>{label}</FieldLabel>{hint && <FieldDescription className="text-xs leading-4 text-pretty">{hint}</FieldDescription>}</div>{children}</FieldPrimitive>
}

export function FormError({ message }: { message: string }) {
  return message ? <FieldError className="rounded-lg bg-danger-subtle px-3 py-2.5 leading-5 text-danger-text">{message}</FieldError> : null
}

export function EmptyGrid({ title, description, actionLabel, onAction, action }: { title: string; description: string; actionLabel: string; onAction: () => void; action?: React.ReactNode }) {
  return (
    <Card className="py-0"><Empty className="py-16"><EmptyMedia variant="icon"><Plus aria-hidden="true" /></EmptyMedia><EmptyHeader><EmptyTitle className="text-lg">{title}</EmptyTitle><EmptyDescription>{description}</EmptyDescription></EmptyHeader><EmptyContent>{action ?? <Button onClick={onAction}><Plus aria-hidden="true" />{actionLabel}</Button>}</EmptyContent></Empty></Card>
  )
}

export function LoadingState() { return <Empty className="min-h-[60vh]" role="status"><EmptyMedia variant="icon"><Spinner className="size-5 text-accent-text" /></EmptyMedia><EmptyHeader><EmptyTitle>正在加载网关数据…</EmptyTitle><EmptyDescription>这通常只需要几秒钟。</EmptyDescription></EmptyHeader></Empty> }

export function ErrorState({ message, onRetry }: { message: string; onRetry: () => void }) { return <div className="mx-auto flex min-h-[60vh] max-w-md flex-col items-center justify-center text-center" role="alert"><span className="grid size-12 place-items-center rounded-2xl bg-danger-subtle text-danger-text"><Zap className="size-5" aria-hidden="true" /></span><h1 className="mt-5 text-xl font-semibold">无法打开控制台</h1><p className="mt-2 text-sm leading-6 text-muted-foreground">{message}</p><Button className="mt-6" onClick={onRetry}><RotateCw aria-hidden="true" />重新加载</Button></div> }
