"use client"

import { useState } from "react"
import { consoleAPI } from "@/lib/api"
import { toast } from "sonner"
import { AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"

export function DeleteProviderDialog({ provider, onClose, onDeleted }: { provider: { id: number; name: string }; onClose: () => void; onDeleted: () => void }) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const remove = async () => {
    setBusy(true); setError("")
    try { await consoleAPI.providerDelete(provider.id); toast.success("供应商已删除"); onDeleted() }
    catch (error) { setError(error instanceof Error ? error.message : "删除失败，请重试。"); setBusy(false) }
  }
  return <AlertDialog open onOpenChange={open => { if (!open && !busy) onClose() }}><AlertDialogContent className="glass-popup">
    <AlertDialogHeader><AlertDialogTitle>删除“{provider.name}”？</AlertDialogTitle><AlertDialogDescription>将删除供应商、其中的账户凭证及关联的模型路由。此操作无法恢复。如果只需移除一个授权，请进入供应商管理删除该账户。</AlertDialogDescription></AlertDialogHeader>
    {error && <p role="alert" className="text-sm text-danger-text">{error}</p>}
    <AlertDialogFooter><Button variant="outline" disabled={busy} onClick={onClose}>保留供应商</Button><Button variant="destructive" disabled={busy} onClick={remove}>{busy ? "正在删除" : "删除整个供应商"}</Button></AlertDialogFooter>
  </AlertDialogContent></AlertDialog>
}
