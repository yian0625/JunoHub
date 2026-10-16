"use client"

import { useId, useRef, useState } from "react"
import { Camera, Trash2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { ProviderAvatar } from "@/components/provider-avatar"
import { compressAvatar } from "@/lib/avatar-image"
import { cn } from "@/lib/utils"

export function ProviderAvatarEditor({ avatarUrl, onChange, onProcessingChange, disabled = false }: { avatarUrl: string; onChange: (value: string) => void; onProcessingChange?: (processing: boolean) => void; disabled?: boolean }) {
  const fileInputRef = useRef<HTMLInputElement>(null)
  const hintId = useId()
  const [dragging, setDragging] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")

  const accept = async (file?: File) => {
    if (!file || disabled || busy) return
    setError("")
    setBusy(true)
    onProcessingChange?.(true)
    try { onChange(await compressAvatar(file)) }
    catch (cause) { setError(cause instanceof Error ? cause.message : "无法读取头像图片。") }
    finally { setBusy(false); onProcessingChange?.(false) }
  }

  return <div className="space-y-2">
    <p className="text-sm font-medium">供应商头像</p>
    <div className="flex flex-wrap items-center gap-3">
      <span className="grid size-14 shrink-0 place-items-center overflow-hidden rounded-xl border border-border-default bg-bg-raised text-foreground">
        <ProviderAvatar provider={{ provider_type: "openai-compatible", avatar_url: avatarUrl }} size={24} />
      </span>
      <div id={hintId} className="min-w-0 flex-1 text-xs leading-5 text-muted-foreground">选择、拖放或粘贴 PNG、JPEG、WebP 图片，自动裁成正方形，最大 8MB。</div>
      {avatarUrl && <Button type="button" variant="ghost" size="sm" disabled={disabled || busy} onClick={() => { onChange(""); setError("") }}><Trash2 aria-hidden="true" />移除</Button>}
    </div>
    <input ref={fileInputRef} type="file" accept="image/png,image/jpeg,image/webp" className="hidden" tabIndex={-1} disabled={disabled || busy}
      onChange={event => { const file = event.currentTarget.files?.[0]; event.currentTarget.value = ""; void accept(file) }} />
    <button type="button" disabled={disabled || busy} aria-describedby={hintId} aria-busy={busy}
      className={cn("flex min-h-20 w-full flex-col items-center justify-center gap-1 rounded-xl border border-dashed px-4 py-4 text-center text-sm outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50", dragging ? "border-accent-text bg-accent-subtle" : "border-border-strong bg-bg-sunken hover:border-accent-text hover:bg-accent-subtle")}
      onClick={() => fileInputRef.current?.click()}
      onDragOver={event => { event.preventDefault(); if (!disabled && !busy) setDragging(true) }}
      onDragLeave={() => setDragging(false)}
      onDrop={event => { event.preventDefault(); setDragging(false); void accept(event.dataTransfer.files[0]) }}
      onPaste={event => { const file = Array.from(event.clipboardData.items).find(item => item.type.startsWith("image/"))?.getAsFile(); if (file) { event.preventDefault(); void accept(file) } }}>
      <Camera className="size-4 text-accent-text" aria-hidden="true" />
      {busy ? "正在处理图片…" : "点击选择图片，也可拖放或粘贴"}
    </button>
    {error && <p role="alert" className="text-sm text-danger-text">{error}</p>}
  </div>
}
