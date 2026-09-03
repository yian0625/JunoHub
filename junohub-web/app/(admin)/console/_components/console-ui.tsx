"use client"

import { Plus, RotateCw, Trash2, Zap } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Label } from "@/components/ui/label"
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

export function MetricCard({ label, value, note, icon: Icon }: { label: string; value: number; note: string; icon: React.ElementType }) {
  return <div className="surface-card rounded-2xl p-5"><div className="flex items-center justify-between gap-3"><p className="text-xs font-medium text-muted-foreground">{label}</p><span className="grid size-8 place-items-center rounded-lg bg-accent-subtle text-accent-text"><Icon className="size-4" strokeWidth={1.75} aria-hidden="true" /></span></div><p className="mt-4 font-mono text-3xl font-medium tracking-[-.06em] tabular-nums">{value}</p><p className="mt-1 text-xs text-muted-foreground">{note}</p></div>
}

export function ResourceGrid({ children }: { children: React.ReactNode }) {
  return <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">{children}</div>
}

export function ResourceCard({ badge, title, meta, status, actions, onDelete, deleteLabel }: { badge: React.ReactNode; title: string; meta: string; status: { active: boolean; label: string }; actions?: React.ReactNode; onDelete: () => void; deleteLabel: string }) {
  return (
    <article className="surface-card flex min-h-[11.5rem] flex-col rounded-2xl p-5 transition-shadow duration-200 hover:shadow-[var(--glass-highlight),var(--shadow-surface-hover)]">
      <div className="flex items-start justify-between gap-3">
        <span className="grid size-10 shrink-0 place-items-center rounded-xl bg-[var(--glass-bg-subtle)] text-accent-text ring-1 ring-inset ring-[var(--glass-border)]">{badge}</span>
        <Button size="icon" variant="ghost" onClick={onDelete} className="-me-1.5 -mt-1.5 size-8 text-muted-foreground hover:bg-danger-subtle hover:text-danger-text" aria-label={deleteLabel}><Trash2 aria-hidden="true" /></Button>
      </div>
      <h3 className="mt-4 truncate text-[15px] font-semibold leading-snug" title={title}>{title}</h3>
      <p className="mt-1.5 truncate font-mono text-xs leading-5 text-muted-foreground" title={meta}>{meta}</p>
      <div className="mt-auto flex items-center justify-between gap-3 border-t border-[var(--glass-border-subtle)] pt-4">
        <span className="inline-flex items-center gap-2 text-xs text-muted-foreground">
          <span className={cn("size-1.5 rounded-full", status.active ? "bg-success-solid" : "bg-text-disabled")} aria-hidden="true" />
          {status.label}
        </span>
        {actions}
      </div>
    </article>
  )
}

export function AddCard({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} className="group flex min-h-[11.5rem] flex-col items-center justify-center gap-3 rounded-2xl border border-dashed border-border-default bg-transparent p-5 text-muted-foreground transition-colors duration-200 hover:border-accent-solid/40 hover:bg-[var(--glass-bg-subtle)] hover:text-accent-text focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
      <span className="grid size-10 place-items-center rounded-full ring-1 ring-inset ring-border-default transition-colors duration-200 group-hover:ring-accent-solid/40"><Plus className="size-5" strokeWidth={1.75} aria-hidden="true" /></span>
      <span className="text-sm font-medium">{label}</span>
    </button>
  )
}

export function FormDialog({ open, onOpenChange, title, description, children }: { open: boolean; onOpenChange: (open: boolean) => void; title: string; description: string; children: React.ReactNode }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="glass-popup gap-0 rounded-2xl p-0 sm:max-w-md">
        <DialogHeader className="px-6 pb-4 pt-6">
          <DialogTitle className="text-base font-semibold">{title}</DialogTitle>
          <DialogDescription className="text-sm leading-6">{description}</DialogDescription>
        </DialogHeader>
        <div className="px-6 pb-6">{children}</div>
      </DialogContent>
    </Dialog>
  )
}

export function Field({ id, label, hint, children }: { id: string; label: string; hint?: string; children: React.ReactNode }) {
  return <div className="space-y-2"><div className="flex items-center justify-between gap-3"><Label htmlFor={id} className="text-sm font-medium">{label}</Label>{hint && <span className="text-xs text-muted-foreground">{hint}</span>}</div>{children}</div>
}

export function FormError({ message }: { message: string }) {
  return message ? <p role="alert" className="rounded-xl bg-danger-subtle px-3 py-2.5 text-sm leading-5 text-danger-text">{message}</p> : null
}

export function EmptyGrid({ title, description, actionLabel, onAction }: { title: string; description: string; actionLabel: string; onAction: () => void }) {
  return (
    <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-border-default px-6 py-16 text-center">
      <p className="text-[15px] font-semibold">{title}</p>
      <p className="mt-2 max-w-sm text-sm leading-6 text-muted-foreground">{description}</p>
      <Button className="mt-6" onClick={onAction}><Plus aria-hidden="true" />{actionLabel}</Button>
    </div>
  )
}

export function LoadingState() { return <div className="flex min-h-[60vh] flex-col items-center justify-center" role="status"><RotateCw className="size-6 animate-spin text-accent-text" aria-hidden="true" /><p className="mt-4 text-sm font-medium">正在加载网关数据…</p><p className="mt-1 text-sm text-muted-foreground">这通常只需要几秒钟。</p></div> }

export function ErrorState({ message, onRetry }: { message: string; onRetry: () => void }) { return <div className="mx-auto flex min-h-[60vh] max-w-md flex-col items-center justify-center text-center" role="alert"><span className="grid size-12 place-items-center rounded-2xl bg-danger-subtle text-danger-text"><Zap className="size-5" aria-hidden="true" /></span><h1 className="mt-5 text-xl font-semibold">无法打开控制台</h1><p className="mt-2 text-sm leading-6 text-muted-foreground">{message}</p><Button className="mt-6" onClick={onRetry}><RotateCw aria-hidden="true" />重新加载</Button></div> }
