"use client"

import { useEffect, useMemo, useState } from "react"
import { useRouter } from "next/navigation"
import { Search, Shield, UserCheck, Users, UserX, RotateCw } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { ConfirmDialog } from "@/components/confirm-dialog"
import { adminUserAPI } from "@/lib/api"
import { useAuthStore } from "@/lib/store"
import { toast } from "sonner"

interface UserRow { id: number; email: string; display_name: string; role: "admin" | "user"; status: number; create_time: number }
type UserAction = { type: "status" | "role"; user: UserRow }

export default function AdminPage() {
  const router = useRouter()
  const user = useAuthStore(state => state.user)
  const [users, setUsers] = useState<UserRow[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState("")
  const [query, setQuery] = useState("")
  const [action, setAction] = useState<UserAction | null>(null)
  const [updating, setUpdating] = useState<number | null>(null)

  const load = async () => {
    setLoading(true); setError("")
    try { const data = await adminUserAPI.list(); setUsers(data.list || []) }
    catch (cause) { const message = cause instanceof Error ? cause.message : "无法加载用户，请检查网络后重试。"; setError(message); toast.error(message) }
    finally { setLoading(false) }
  }

  useEffect(() => {
    if (user && user.role !== "admin") { router.replace("/console"); return }
    if (user?.role === "admin") load()
  }, [user, router])

  const filteredUsers = useMemo(() => {
    const value = query.trim().toLowerCase()
    if (!value) return users
    return users.filter(item => `${item.display_name} ${item.email} ${item.id}`.toLowerCase().includes(value))
  }, [query, users])

  const confirmAction = async () => {
    if (!action) return
    const item = action.user
    setUpdating(item.id)
    try {
      if (action.type === "status") {
        await adminUserAPI.update({ id: item.id, status: item.status === 1 ? 0 : 1 })
        toast.success(item.status === 1 ? "账户已停用" : "账户已启用")
      } else {
        const role = item.role === "admin" ? "user" : "admin"
        await adminUserAPI.update({ id: item.id, role })
        toast.success(role === "admin" ? "已授予管理权限" : "已移除管理权限")
      }
      setAction(null); await load()
    } catch (cause) { toast.error(cause instanceof Error ? cause.message : "无法更新账户，请稍后重试。") }
    finally { setUpdating(null) }
  }

  const activeCount = users.filter(item => item.status === 1).length
  const adminCount = users.filter(item => item.role === "admin").length
  const targetName = action?.user.display_name || action?.user.email || "此用户"
  const removesAccess = action?.type === "status" ? action.user.status === 1 : action?.user.role === "admin"
  const actionTitle = action?.type === "status" ? `${action.user.status === 1 ? "停用" : "启用"}“${targetName}”？` : `${action?.user.role === "admin" ? "移除" : "授予"}“${targetName}”的管理权限？`
  const actionDescription = action?.type === "status" && action.user.status === 1 ? "停用后，该用户将无法登录，现有访问密钥也会立即失效。" : action?.type === "role" && action.user.role === "admin" ? "该用户将无法继续管理其他账户，但仍可使用个人模型网关。" : "此更改会立即生效，你可以稍后再次调整。"

  return (
    <div className="view-enter space-y-8">
      <header><h1 className="page-heading">用户管理</h1><p className="page-lead mt-2.5">查看工作区成员，管理账户状态和系统权限。</p></header>

      <section className="grid grid-cols-3 gap-4" aria-label="用户概览">
        <SummaryMetric label="用户总数" value={users.length} icon={Users} />
        <SummaryMetric label="正常账户" value={activeCount} icon={UserCheck} />
        <SummaryMetric label="管理员" value={adminCount} icon={Shield} />
      </section>

      <Card>
        <CardHeader className="gap-4 sm:grid-cols-[1fr_minmax(240px,340px)] sm:items-end">
          <div><CardTitle>注册用户</CardTitle><CardDescription className="mt-1.5">账户状态会同时影响登录和访问密钥。</CardDescription></div>
          <label className="relative block"><span className="sr-only">搜索用户</span><Search className="pointer-events-none absolute inset-inline-start-3.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" /><Input name="user-search" type="search" value={query} onChange={event => setQuery(event.target.value)} placeholder="搜索姓名、邮箱或 ID" className="ps-10" /></label>
        </CardHeader>
        <CardContent className="px-0 pb-0 sm:px-0">
          {loading ? <div className="flex min-h-64 items-center justify-center gap-3" role="status"><RotateCw className="size-5 animate-spin text-accent-text" aria-hidden="true" /><span className="text-sm text-muted-foreground">正在加载用户…</span></div>
          : error ? <div className="flex min-h-64 flex-col items-center justify-center px-6 text-center" role="alert"><p className="text-sm font-medium">无法加载用户</p><p className="mt-1 max-w-md text-sm leading-6 text-muted-foreground">{error}</p><Button className="mt-5" onClick={load}><RotateCw aria-hidden="true" />重新加载</Button></div>
          : filteredUsers.length === 0 ? <div className="flex min-h-64 flex-col items-center justify-center px-6 text-center"><Search className="size-6 text-text-disabled" aria-hidden="true" /><p className="mt-4 text-sm font-semibold">没有匹配的用户</p><p className="mt-1 text-sm text-muted-foreground">请尝试其他姓名、邮箱或 ID。</p>{query && <Button variant="outline" className="mt-5" onClick={() => setQuery("")}>清除搜索</Button>}</div>
          : <div className="divide-y divide-border-subtle border-t border-border-subtle">{filteredUsers.map(item => <UserItem key={item.id} item={item} busy={updating === item.id} onAction={type => setAction({ type, user: item })} />)}</div>}
        </CardContent>
      </Card>

      <ConfirmDialog open={Boolean(action)} onOpenChange={open => { if (!open) setAction(null) }} title={actionTitle} description={actionDescription} confirmText={action?.type === "status" ? (action.user.status === 1 ? "停用账户" : "启用账户") : (action?.user.role === "admin" ? "移除权限" : "授予权限")} cancelText="取消" destructive={removesAccess} onConfirm={confirmAction} />
    </div>
  )
}

function SummaryMetric({ label, value, icon: Icon }: { label: string; value: number; icon: React.ElementType }) {
  return <div className="surface-card rounded-2xl p-5"><div className="flex items-center justify-between gap-2"><p className="text-xs font-medium text-muted-foreground">{label}</p><span className="grid size-8 place-items-center rounded-lg bg-accent-subtle text-accent-text"><Icon className="size-4" strokeWidth={1.75} aria-hidden="true" /></span></div><p className="mt-4 font-mono text-3xl font-medium tracking-[-.06em] tabular-nums">{value}</p></div>
}

function UserItem({ item, busy, onAction }: { item: UserRow; busy: boolean; onAction: (type: "status" | "role") => void }) {
  const name = item.display_name || item.email
  return (
    <article className="flex flex-col gap-4 px-5 py-5 transition-colors duration-150 hover:bg-bg-sunken/60 sm:px-6 lg:flex-row lg:items-center lg:justify-between">
      <div className="flex min-w-0 items-start gap-3.5"><span className="grid size-9 shrink-0 place-items-center rounded-full bg-accent-solid text-sm font-semibold text-text-inverse" aria-hidden="true">{name.slice(0, 1).toUpperCase()}</span><div className="min-w-0"><p className="break-words text-sm font-semibold">{name}</p><p className="mt-1 break-all font-mono text-xs text-muted-foreground"><bdi>{item.email}</bdi></p><p className="mt-1 text-[11px] text-muted-foreground">用户 ID {item.id} · 注册于 {new Date(item.create_time * 1000).toLocaleDateString("zh-CN")}</p></div></div>
      <div className="flex flex-wrap items-center gap-2 ps-[54px] lg:justify-end lg:ps-0"><span className={item.status === 1 ? "inline-flex items-center gap-1.5 rounded-full bg-success-subtle px-2.5 py-1 text-xs font-medium text-success-text ring-1 ring-inset ring-success-solid/25" : "inline-flex items-center gap-1.5 rounded-full bg-bg-sunken px-2.5 py-1 text-xs font-medium text-muted-foreground ring-1 ring-inset ring-border-default"}>{item.status === 1 ? <UserCheck className="size-3.5" aria-hidden="true" /> : <UserX className="size-3.5" aria-hidden="true" />}{item.status === 1 ? "正常" : "已停用"}</span><span className={item.role === "admin" ? "inline-flex items-center gap-1.5 rounded-full bg-accent-subtle px-2.5 py-1 text-xs font-medium text-accent-text ring-1 ring-inset ring-accent-solid/20" : "inline-flex items-center gap-1.5 rounded-full bg-bg-sunken px-2.5 py-1 text-xs font-medium text-muted-foreground ring-1 ring-inset ring-border-default"}><Shield className="size-3.5" aria-hidden="true" />{item.role === "admin" ? "管理员" : "普通用户"}</span><Button size="sm" variant="outline" onClick={() => onAction("role")} disabled={busy}>{item.role === "admin" ? "移除权限" : "授予权限"}</Button><Button size="sm" variant={item.status === 1 ? "destructive" : "outline"} onClick={() => onAction("status")} disabled={busy}>{busy && <RotateCw className="animate-spin" aria-hidden="true" />}{item.status === 1 ? "停用账户" : "启用账户"}</Button></div>
    </article>
  )
}
