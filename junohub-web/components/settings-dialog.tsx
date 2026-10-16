"use client"

import { AppForm } from "@/components/ui/app-form"

import { useRef, useState } from "react"
import { useRouter } from "next/navigation"
import { Camera, KeyRound, RotateCw, Trash2, UserRound } from "lucide-react"
import { toast } from "sonner"
import { accountAPI } from "@/lib/api"
import { useAuthStore } from "@/lib/store"
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { compressAvatar } from "@/lib/avatar-image"

export function SettingsDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const router = useRouter()
  const logout = useAuthStore(state => state.logout)
  const user = useAuthStore(state => state.user)
  const updateUser = useAuthStore(state => state.updateUser)
  const [avatarEditorOpen, setAvatarEditorOpen] = useState(false)
  const [dragging, setDragging] = useState(false)
  const initialName = user?.nickname || user?.email || ""
  const [displayName, setDisplayName] = useState(initialName)
  const [avatarUrl, setAvatarUrl] = useState(user?.avatar_url || "")
  const [profileError, setProfileError] = useState("")
  const [profileBusy, setProfileBusy] = useState(false)
  const [avatarProcessing, setAvatarProcessing] = useState(false)
  const avatarProcessingRef = useRef(false)
  const [currentPassword, setCurrentPassword] = useState("")
  const [newPassword, setNewPassword] = useState("")
  const [confirmPassword, setConfirmPassword] = useState("")
  const [passwordError, setPasswordError] = useState("")
  const [passwordBusy, setPasswordBusy] = useState(false)

  const chooseAvatar = async (file?: File) => {
    if (!file || profileBusy || avatarProcessingRef.current) return
    avatarProcessingRef.current = true
    setAvatarProcessing(true)
    try {
      setProfileError("")
      setAvatarUrl(await compressAvatar(file))
      setAvatarEditorOpen(false)
    } catch (error) {
      setProfileError(error instanceof Error ? error.message : "无法读取头像，请选择其他图片。")
    } finally { avatarProcessingRef.current = false; setAvatarProcessing(false) }
  }

  const saveProfile = async (event: React.FormEvent) => {
    event.preventDefault()
    if (profileBusy || avatarProcessingRef.current) return
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
      toast.success("密码已更新，请重新登录")
      onOpenChange(false)
      logout()
      router.replace("/login")
    } catch (error) {
      setPasswordError(error instanceof Error ? error.message : "无法更新密码。")
    } finally { setPasswordBusy(false) }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="glass-popup gap-0 overflow-x-hidden overflow-y-auto rounded-2xl p-0 sm:max-w-xl">
        <DialogHeader className="border-b border-[var(--glass-border-subtle)] px-6 pb-5 pt-6">
          <DialogTitle className="text-base font-semibold">设置</DialogTitle>
          <DialogDescription>管理个人资料和账户安全。</DialogDescription>
        </DialogHeader>

        <div className="divide-y divide-[var(--glass-border-subtle)]">
          <AppForm onSubmit={saveProfile} className="px-6 py-5" aria-busy={profileBusy}>
            <div className="flex items-center gap-2.5"><UserRound className="size-4 text-accent-text" aria-hidden="true" /><h2 className="text-sm font-semibold">个人资料</h2></div>
            <div className="mt-4 flex flex-wrap items-center gap-4">
              <Avatar className="size-16">
                {avatarUrl && <AvatarImage src={avatarUrl} alt="当前头像" />}
                <AvatarFallback className="text-lg font-semibold text-foreground">{initialName.slice(0, 1).toUpperCase()}</AvatarFallback>
              </Avatar>
              <div className="flex flex-wrap gap-2">
                <Button type="button" variant="outline" size="sm" disabled={profileBusy || avatarProcessing} aria-expanded={avatarEditorOpen} aria-controls="avatar-dropzone" onClick={() => setAvatarEditorOpen(!avatarEditorOpen)}><Camera aria-hidden="true" />更换头像</Button>
                {avatarUrl && <Button type="button" variant="ghost" size="sm" disabled={profileBusy || avatarProcessing} className="text-muted-foreground hover:bg-danger-subtle hover:text-danger-text" onClick={() => setAvatarUrl("")}><Trash2 aria-hidden="true" />移除</Button>}
              </div>
            </div>
            {avatarEditorOpen && <div id="avatar-dropzone" role="region" aria-label="上传头像" tabIndex={0}
              className={`mt-4 rounded-xl border border-dashed p-5 text-center outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring ${dragging ? "border-accent-text bg-accent-subtle" : "border-border-strong bg-bg-sunken"}`}
              onDragOver={event => { event.preventDefault(); setDragging(true) }}
              onDragLeave={() => setDragging(false)}
              onDrop={event => { event.preventDefault(); setDragging(false); void chooseAvatar(event.dataTransfer.files[0]) }}
              onPaste={event => { const file = Array.from(event.clipboardData.items).find(item => item.type.startsWith("image/"))?.getAsFile(); if (file) { event.preventDefault(); void chooseAvatar(file) } }}>
              <Camera className="mx-auto mb-2 size-5 text-accent-text" aria-hidden="true" />
              <p className="text-sm font-medium">拖入图片，或聚焦此区域后粘贴图片</p>
              <p className="mt-1 text-xs leading-5 text-muted-foreground">PNG、JPEG、WebP，最大 8MB · 自动居中裁剪</p>
            </div>}
            <div className="mt-5 space-y-2"><Label htmlFor="settings-display-name">显示名称</Label><Input id="settings-display-name" name="display-name" autoComplete="name" value={displayName} onChange={event => { setDisplayName(event.target.value); setProfileError("") }} maxLength={64} /></div>
            <div className="mt-4 space-y-2"><Label htmlFor="settings-email">邮箱</Label><Input id="settings-email" value={user?.email || ""} readOnly disabled /></div>
            {profileError && <p role="alert" className="mt-4 rounded-xl bg-danger-subtle px-3 py-2.5 text-sm text-danger-text">{profileError}</p>}
            <div className="mt-5 flex justify-end"><Button type="submit" disabled={profileBusy || avatarProcessing}>{(profileBusy || avatarProcessing) && <RotateCw className="animate-spin" aria-hidden="true" />}保存资料</Button></div>
          </AppForm>

          <AppForm onSubmit={savePassword} className="px-6 py-5" aria-busy={passwordBusy}>
            <div className="flex items-center gap-2.5"><KeyRound className="size-4 text-accent-text" aria-hidden="true" /><h2 className="text-sm font-semibold">修改密码</h2></div>
            <div className="mt-4 space-y-4">
              <div className="space-y-2"><Label htmlFor="settings-current-password">当前密码</Label><Input id="settings-current-password" type="password" autoComplete="current-password" value={currentPassword} onChange={event => { setCurrentPassword(event.target.value); setPasswordError("") }} /></div>
              <div className="space-y-2"><Label htmlFor="settings-new-password">新密码</Label><Input id="settings-new-password" type="password" autoComplete="new-password" value={newPassword} onChange={event => { setNewPassword(event.target.value); setPasswordError("") }} /></div>
              <div className="space-y-2"><Label htmlFor="settings-confirm-password">确认新密码</Label><Input id="settings-confirm-password" type="password" autoComplete="new-password" value={confirmPassword} onChange={event => { setConfirmPassword(event.target.value); setPasswordError("") }} /></div>
            </div>
            {passwordError && <p role="alert" className="mt-4 rounded-xl bg-danger-subtle px-3 py-2.5 text-sm text-danger-text">{passwordError}</p>}
            <div className="mt-5 flex justify-end"><Button type="submit" variant="outline" disabled={passwordBusy}>{passwordBusy && <RotateCw className="animate-spin" aria-hidden="true" />}更新密码</Button></div>
          </AppForm>
        </div>
      </DialogContent>
    </Dialog>
  )
}
