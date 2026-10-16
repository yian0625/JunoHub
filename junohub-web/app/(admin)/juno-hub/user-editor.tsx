"use client"

import { useState } from "react"
import { toast } from "sonner"
import { adminUserAPI } from "@/lib/api"
import { Button } from "@/components/coss-ui/button"
import { SelectField } from "@/components/ui/select-field"
import { AppForm } from "@/components/ui/app-form"
import { RequestLimitsFields, useRequestLimits } from "@/components/resource-limits-fields"
import { Field, FormDialog, FormError } from "../console/_components/console-ui"

type User = { id: number; email: string; display_name: string; role: "admin" | "user"; status: number }
export function UserEditor({ user, onClose, onSaved }: { user: User; onClose: () => void; onSaved: () => void }) {
  const [role, setRole] = useState<string>(user.role)
  const [status, setStatus] = useState(String(user.status))
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const limits = useRequestLimits("user", user.id, true)
  const save = async (event: React.FormEvent) => {
    event.preventDefault()
    if (busy) return
    setBusy(true); setError("")
    try {
      await adminUserAPI.update({ id: user.id, role: role as User["role"], status: Number(status), request_limits: limits.value() })
      toast.success("用户设置已保存"); onSaved()
    } catch (error) { setError(error instanceof Error ? error.message : "无法保存用户设置") }
    finally { setBusy(false) }
  }
  return <FormDialog open onOpenChange={open => { if (!open && !busy) onClose() }} title="编辑用户" description={user.display_name ? `${user.display_name} · ${user.email}` : user.email}>
    <AppForm onSubmit={save} className="space-y-5" aria-busy={busy}>
      <fieldset disabled={busy} className="space-y-5"><legend className="sr-only">用户设置</legend>
        <Field id="user-edit-role" label="用户身份"><SelectField id="user-edit-role" value={role} onValueChange={setRole} options={[{ value: "user", label: "普通用户" }, { value: "admin", label: "管理员" }]} /></Field>
        <Field id="user-edit-status" label="账户状态"><SelectField id="user-edit-status" value={status} onValueChange={setStatus} options={[{ value: "1", label: "正常" }, { value: "0", label: "已停用" }]} /></Field>
        <RequestLimitsFields limits={limits} prefix={`user-edit-${user.id}`} disabled={busy} />
      </fieldset>
      <p className="text-xs leading-5 text-muted-foreground">请求限制对该用户的所有密钥合计生效。修改身份或账户状态会使现有登录会话失效。</p>
      <FormError message={error} />
      <div className="flex justify-end gap-3 border-t border-border-default pt-4"><Button type="button" variant="outline" disabled={busy} onClick={onClose}>取消</Button><Button type="submit" loading={busy}>保存</Button></div>
    </AppForm>
  </FormDialog>
}
