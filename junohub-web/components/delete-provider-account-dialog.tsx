"use client"

import { useState } from "react"
import { RotateCw } from "lucide-react"
import { consoleAPI } from "@/lib/api"
import { toast } from "sonner"
import { AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"

export function DeleteProviderAccountDialog({ providerId, account, lastAccount, onClose, onDeleted }: { providerId: number; account: { id: number; name: string }; lastAccount: boolean; onClose: () => void; onDeleted: (id: number) => void }) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const remove = async () => {
    setBusy(true); setError("")
    try { await consoleAPI.providerAccountDelete(providerId, account.id); onDeleted(account.id); toast.success("授权账户已删除") }
    catch (error) { setError(error instanceof Error ? error.message : "删除失败，请重试。"); setBusy(false) }
  }
  return <AlertDialog open onOpenChange={open => { if (!open && !busy) onClose() }}><AlertDialogContent className="glass-popup">
    <AlertDialogHeader><AlertDialogTitle>删除“{account.name}”？</AlertDialogTitle><AlertDialogDescription>该授权将从当前供应商移除，供应商和模型路由会保留。需要再次使用时，请重新添加授权。</AlertDialogDescription></AlertDialogHeader>
    {lastAccount && <p className="rounded-xl bg-danger-subtle p-3 text-sm leading-6 text-danger-text">这是最后一个账户。删除后，此供应商将暂时无法处理请求，直到添加新的授权。</p>}
    {error && <p role="alert" className="text-sm text-danger-text">{error}</p>}
    <AlertDialogFooter><Button type="button" variant="outline" disabled={busy} onClick={onClose}>保留账户</Button><Button type="button" disabled={busy} onClick={remove} className="bg-danger-subtle text-danger-text hover:bg-danger-subtle/80">{busy && <RotateCw className="animate-spin" aria-hidden="true" />}{busy ? "正在删除" : "删除此账户"}</Button></AlertDialogFooter>
  </AlertDialogContent></AlertDialog>
}
