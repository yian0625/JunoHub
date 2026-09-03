"use client"

import { useRef, useState } from "react"
import { Camera, KeyRound, RotateCw, Trash2, UserRound } from "lucide-react"
import { toast } from "sonner"
import { accountAPI } from "@/lib/api"
import { useAuthStore } from "@/lib/store"
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"

export function SettingsDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const user = useAuthStore(state => state.user)
  const updateUser = useAuthStore(state => state.updateUser)
  const fileRef = useRef<HTMLInputElement>(null)
  const initialName = user?.nickname || user?.email || ""
  const [displayName, setDisplayName] = useState(initialName)
  const [avatarUrl, setAvatarUrl] = useState(user?.avatar_url || "")
  const [profileError, setProfileError] = useState("")
  const [profileBusy, setProfileBusy] = useState(false)
  const [currentPassword, setCurrentPassword] = useState("")
  const [newPassword, setNewPassword] = useState("")
  const [confirmPassword, setConfirmPassword] = useState("")
  const [passwordError, setPasswordError] = useState("")
  const [passwordBusy, setPasswordBusy] = useState(false)

  const chooseAvatar = async (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0]
    event.target.value = ""
    if (!file) return
    try {
      setProfileError("")
      setAvatarUrl(await compressAvatar(file))
    } catch (error) {
      setProfileError(error instanceof Error ? error.message : "无法读取头像，请选择其他图片。")
    }
  }

  const saveProfile = async (event: React.FormEvent) => {
    event.preventDefault()
    const name = displayName.trim()
    if (!name) { setProfileError("请输入显示名称。"); return }
    if ([...name].length > 64) { setProfileError("显示名称不能超过 64 个字符。"); return }
    setProfileBusy(true); setProfileError("")
    try {
      const profile = await accountAPI.updateProfile({ display_name: name, avatar_url: avatarUrl })
      setDisplayName(profile.display_name)
      setAvatarUrl(profile.avatar_url || "")
      updateUser({ nickname: profile.display_name, email: profile.email, avatar_url: profile.avatar_url || "", role: profile.role })
      toast.success("个人资料已更新")
    } catch (error) {
      setProfileError(error instanceof Error ? error.message : "无法更新个人资料。")
    } finally { setProfileBusy(false) }
  }

  const savePassword = async (event: React.FormEvent) => {
    event.preventDefault()
    if (!currentPassword) { setPasswordError("请输入当前密码。"); return }
    if (newPassword.length < 8) { setPasswordError("新密码至少需要 8 个字符。"); return }
    if (newPassword !== confirmPassword) { setPasswordError("两次输入的新密码不一致。"); return }
    setPasswordBusy(true); setPasswordError("")
    try {
      await accountAPI.updatePassword({ current_password: currentPassword, new_password: newPassword })
      setCurrentPassword(""); setNewPassword(""); setConfirmPassword("")
      toast.success("密码已更新")
    } catch (error) {
      setPasswordError(error instanceof Error ? error.message : "无法更新密码。")
    } finally { setPasswordBusy(false) }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="glass-popup gap-0 overflow-hidden rounded-2xl p-0 sm:max-w-xl">
        <DialogHeader className="border-b border-[var(--glass-border-subtle)] px-6 pb-5 pt-6">
          <DialogTitle className="text-base font-semibold">设置</DialogTitle>
          <DialogDescription>管理个人资料和账户安全。</DialogDescription>
        </DialogHeader>

        <div className="divide-y divide-[var(--glass-border-subtle)]">
          <form onSubmit={saveProfile} className="px-6 py-5" aria-busy={profileBusy}>
            <div className="flex items-center gap-2.5"><UserRound className="size-4 text-accent-text" aria-hidden="true" /><h2 className="text-sm font-semibold">个人资料</h2></div>
            <div className="mt-4 flex flex-wrap items-center gap-4">
              <Avatar className="size-16">
                {avatarUrl && <AvatarImage src={avatarUrl} alt="当前头像" />}
                <AvatarFallback className="text-lg font-semibold text-foreground">{initialName.slice(0, 1).toUpperCase()}</AvatarFallback>
              </Avatar>
              <div className="flex flex-wrap gap-2">
                <input ref={fileRef} type="file" accept="image/png,image/jpeg,image/webp" onChange={chooseAvatar} className="sr-only" aria-label="选择头像图片" />
                <Button type="button" variant="outline" size="sm" onClick={() => fileRef.current?.click()}><Camera aria-hidden="true" />更换头像</Button>
                {avatarUrl && <Button type="button" variant="ghost" size="sm" className="text-muted-foreground hover:bg-danger-subtle hover:text-danger-text" onClick={() => setAvatarUrl("")}><Trash2 aria-hidden="true" />移除</Button>}
              </div>
            </div>
            <div className="mt-5 space-y-2"><Label htmlFor="settings-display-name">显示名称</Label><Input id="settings-display-name" name="display-name" autoComplete="name" value={displayName} onChange={event => { setDisplayName(event.target.value); setProfileError("") }} maxLength={64} /></div>
            <div className="mt-4 space-y-2"><Label htmlFor="settings-email">邮箱</Label><Input id="settings-email" value={user?.email || ""} readOnly disabled /></div>
            {profileError && <p role="alert" className="mt-4 rounded-xl bg-danger-subtle px-3 py-2.5 text-sm text-danger-text">{profileError}</p>}
            <div className="mt-5 flex justify-end"><Button type="submit" disabled={profileBusy}>{profileBusy && <RotateCw className="animate-spin" aria-hidden="true" />}保存资料</Button></div>
          </form>

          <form onSubmit={savePassword} className="px-6 py-5" aria-busy={passwordBusy}>
            <div className="flex items-center gap-2.5"><KeyRound className="size-4 text-accent-text" aria-hidden="true" /><h2 className="text-sm font-semibold">修改密码</h2></div>
            <div className="mt-4 space-y-4">
              <div className="space-y-2"><Label htmlFor="settings-current-password">当前密码</Label><Input id="settings-current-password" type="password" autoComplete="current-password" value={currentPassword} onChange={event => { setCurrentPassword(event.target.value); setPasswordError("") }} /></div>
              <div className="space-y-2"><Label htmlFor="settings-new-password">新密码</Label><Input id="settings-new-password" type="password" autoComplete="new-password" value={newPassword} onChange={event => { setNewPassword(event.target.value); setPasswordError("") }} /></div>
              <div className="space-y-2"><Label htmlFor="settings-confirm-password">确认新密码</Label><Input id="settings-confirm-password" type="password" autoComplete="new-password" value={confirmPassword} onChange={event => { setConfirmPassword(event.target.value); setPasswordError("") }} /></div>
            </div>
            {passwordError && <p role="alert" className="mt-4 rounded-xl bg-danger-subtle px-3 py-2.5 text-sm text-danger-text">{passwordError}</p>}
            <div className="mt-5 flex justify-end"><Button type="submit" variant="outline" disabled={passwordBusy}>{passwordBusy && <RotateCw className="animate-spin" aria-hidden="true" />}更新密码</Button></div>
          </form>
        </div>
      </DialogContent>
    </Dialog>
  )
}

async function compressAvatar(file: File): Promise<string> {
  if (!/image\/(png|jpeg|webp)/.test(file.type)) throw new Error("请选择 PNG、JPEG 或 WebP 图片。")
  if (file.size > 8 * 1024 * 1024) throw new Error("原始图片不能超过 8MB。")
  const bitmap = await createImageBitmap(file)
  const canvas = document.createElement("canvas")
  canvas.width = 256; canvas.height = 256
  const context = canvas.getContext("2d")
  if (!context) { bitmap.close(); throw new Error("当前浏览器无法处理头像图片。") }
  const side = Math.min(bitmap.width, bitmap.height)
  const sx = (bitmap.width - side) / 2
  const sy = (bitmap.height - side) / 2
  context.drawImage(bitmap, sx, sy, side, side, 0, 0, 256, 256)
  bitmap.close()
  for (const quality of [0.85, 0.7, 0.55]) {
    const blob = await canvasBlob(canvas, quality)
    if (blob.size <= 256 * 1024) return readDataUrl(blob)
  }
  throw new Error("无法将头像压缩到 256KB 以内，请选择其他图片。")
}

function canvasBlob(canvas: HTMLCanvasElement, quality: number): Promise<Blob> {
  return new Promise((resolve, reject) => canvas.toBlob(blob => blob ? resolve(blob) : reject(new Error("无法处理头像图片。")), "image/webp", quality))
}

function readDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result))
    reader.onerror = () => reject(new Error("无法读取头像图片。"))
    reader.readAsDataURL(blob)
  })
}
