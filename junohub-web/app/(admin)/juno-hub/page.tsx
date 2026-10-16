"use client"

import { useEffect, useMemo, useState } from "react"
import { useRouter } from "next/navigation"
import { ChevronDown, Eye, Pencil, SearchIcon, Shield, UserCheck, Users, RotateCw } from "lucide-react"
import { Button } from "@/components/coss-ui/button"
import { Card } from "@/components/ui/card"
import { Badge } from "@/components/coss-ui/badge"
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/coss-ui/input-group"
import { Table, TableBody, TableCaption, TableCell, TableHead, TableHeader, TableRow } from "@/components/coss-ui/table"
import { AppTooltip } from "@/components/ui/tooltip"
import { SelectField } from "@/components/ui/select-field"
import { Spinner } from "@/components/ui/spinner"
import { adminUserAPI } from "@/lib/api"
import { useAuthStore } from "@/lib/store"
import { toast } from "sonner"
import { UserDetail } from "./user-detail"
import { UserEditor } from "./user-editor"
import { PageHead } from "../console/_components/console-ui"

interface UserRow { id: number; email: string; display_name: string; role: "admin" | "user"; status: number; create_time: number }
type RoleFilter = "all" | UserRow["role"]
const roleOptions = [{ value: "all", label: "全部身份" }, { value: "admin", label: "管理员" }, { value: "user", label: "普通用户" }]

export default function AdminPage() {
  const router = useRouter()
  const user = useAuthStore(state => state.user)
  const [users, setUsers] = useState<UserRow[]>([])
  const [total, setTotal] = useState(0)
  const [loadedPage, setLoadedPage] = useState(1)
  const [loadingMore, setLoadingMore] = useState(false)
  const [moreError, setMoreError] = useState("")
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState("")
  const [query, setQuery] = useState("")
  const [roleFilter, setRoleFilter] = useState<RoleFilter>("all")
  const [editing, setEditing] = useState<UserRow | null>(null)
  const [detailId, setDetailId] = useState<number | null>(null)

  const load = async () => {
    setLoading(true); setError(""); setMoreError("")
    try { const data = await adminUserAPI.list(); setUsers(data.list || []); setTotal(data.total); setLoadedPage(1) }
    catch (cause) { const message = cause instanceof Error ? cause.message : "无法加载用户，请检查网络后重试。"; setError(message); toast.error(message) }
    finally { setLoading(false) }
  }

  const loadMore = async () => {
    if (loadingMore || loading) return
    setLoadingMore(true); setMoreError("")
    try {
      const next = loadedPage + 1
      const data = await adminUserAPI.list(next)
      setUsers(current => [...current, ...data.list.filter(item => !current.some(user => user.id === item.id))])
      setTotal(data.total); setLoadedPage(next)
    } catch (cause) { setMoreError(cause instanceof Error ? cause.message : "无法加载更多用户，请重试。") }
    finally { setLoadingMore(false) }
  }

  useEffect(() => {
    if (user && user.role !== "admin") { router.replace("/console"); return }
    if (user?.role === "admin") load()
  }, [user, router])

  const filteredUsers = useMemo(() => {
    const value = query.trim().toLowerCase()
    return users.filter(item => (roleFilter === "all" || item.role === roleFilter) && `${item.display_name} ${item.email}`.toLowerCase().includes(value))
  }, [query, roleFilter, users])
  const hasFilters = Boolean(query.trim()) || roleFilter !== "all"

  const activeCount = users.filter(item => item.status === 1).length
  const adminCount = users.filter(item => item.role === "admin").length
  const partial = users.length < total
  const metricsReady = !loading && !error
  const busy = loading || loadingMore

  return (
    <div className="view-enter space-y-6">
      <PageHead title="用户管理" description="查看工作区成员，管理账户状态和系统权限。" />

      <section className="grid grid-cols-3 gap-3 sm:gap-4" aria-label="用户概览" aria-busy={loading}>
        <SummaryMetric label="用户总数" value={metricsReady ? total : undefined} note="工作区成员" icon={Users} />
        <SummaryMetric label="正常账户" value={metricsReady ? activeCount : undefined} note={partial ? "当前已显示用户" : "可登录的账户"} icon={UserCheck} />
        <SummaryMetric label="管理员" value={metricsReady ? adminCount : undefined} note={partial ? "当前已显示用户" : "可管理工作区"} icon={Shield} />
      </section>

      <section className="space-y-4" aria-labelledby="users-title">
        <div className="flex flex-col gap-3 xl:flex-row xl:items-center xl:justify-between">
          <div><h2 id="users-title" className="text-sm font-semibold">注册用户</h2><p className="mt-1 text-xs text-muted-foreground">账户状态会同时影响登录和访问密钥。</p></div>
          <div className="flex min-w-0 flex-col gap-3 min-[480px]:flex-row min-[480px]:items-center">
            <div className="w-full min-w-0 min-[480px]:w-80">
              <InputGroup>
                <InputGroupInput name="user-search" type="search" value={query} onChange={event => setQuery(event.target.value)} placeholder="搜索姓名或邮箱" aria-label="搜索用户" />
                <InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon>
              </InputGroup>
            </div>
            <div className="w-full min-[480px]:w-40 min-[480px]:shrink-0">
              <label htmlFor="user-role-filter" className="sr-only">用户身份</label>
              <SelectField id="user-role-filter" name="user-role" size="search" value={roleFilter} onValueChange={value => setRoleFilter(value as RoleFilter)} options={roleOptions} />
            </div>
          </div>
        </div>
        {partial && <p className="text-xs text-muted-foreground">搜索和身份筛选作用于当前已显示的用户，可在表格底部加载更多。</p>}
        <Card className="gap-0 overflow-hidden py-0">
          {loading ? <div className="flex h-44 items-center justify-center gap-2 text-muted-foreground" role="status"><Spinner className="size-4" />正在加载用户…</div>
          : error ? <Empty className="py-10" role="alert"><EmptyHeader><EmptyTitle>无法加载用户</EmptyTitle><EmptyDescription>{error}</EmptyDescription></EmptyHeader><EmptyContent><Button variant="outline" onClick={load}><RotateCw aria-hidden="true" />重新加载</Button></EmptyContent></Empty>
          : filteredUsers.length === 0 ? <Empty className="py-10"><EmptyHeader><EmptyMedia variant="icon"><SearchIcon aria-hidden="true" /></EmptyMedia><EmptyTitle>{hasFilters ? "没有匹配的用户" : "暂无用户"}</EmptyTitle><EmptyDescription>{hasFilters ? "调整搜索内容或用户身份，或清除筛选。" : "新注册的工作区成员会显示在这里。"}</EmptyDescription></EmptyHeader>{hasFilters && <EmptyContent><Button variant="outline" onClick={() => { setQuery(""); setRoleFilter("all") }}>清除筛选</Button></EmptyContent>}</Empty>
          : (
          <Table className="min-w-[800px] table-fixed" render={<div role="region" aria-label="用户列表，可横向滚动" tabIndex={0} className="overscroll-x-contain focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring" />}>
            <TableCaption className="sr-only">工作区用户、角色、账户状态与管理操作</TableCaption>
            <TableHeader><TableRow className="bg-bg-sunken/60"><TableHead scope="col" className="px-4">用户</TableHead><TableHead scope="col" className="w-28 px-4">角色</TableHead><TableHead scope="col" className="w-28 px-4">状态</TableHead><TableHead scope="col" className="w-36 px-4">注册时间</TableHead><TableHead scope="col" className="w-32 px-4">操作</TableHead></TableRow></TableHeader>
            <TableBody>
              {filteredUsers.map(item => <UserItem key={item.id} item={item} onDetail={() => setDetailId(item.id)} busy={busy} onEdit={() => setEditing(item)} />)}
            </TableBody>
          </Table>
          )}
          {!loading && !error && filteredUsers.length > 0 && <p className="border-t border-border-default px-4 py-2 text-xs text-muted-foreground sm:hidden">左右滑动查看完整表格</p>}
          {!loading && !error && <div className="flex flex-wrap items-center justify-between gap-3 border-t border-border-default px-4 py-3">
            <p role="status" className="text-xs text-muted-foreground tabular-nums">{hasFilters ? `找到 ${filteredUsers.length} 名用户` : partial ? `已显示 ${users.length} / ${total} 名用户` : `共 ${total} 名用户`}</p>
            {partial && <Button variant="outline" size="sm" loading={loadingMore} disabled={Boolean(editing)} onClick={loadMore}><ChevronDown aria-hidden="true" />加载更多</Button>}
            {moreError && <p role="alert" className="w-full text-xs text-danger-text">{moreError}</p>}
          </div>}
        </Card>
      </section>

      {detailId !== null && <UserDetail key={detailId} id={detailId} onClose={() => setDetailId(null)} />}
      {editing && <UserEditor key={editing.id} user={editing} onClose={() => setEditing(null)} onSaved={() => { setEditing(null); void load() }} />}
    </div>
  )
}

function SummaryMetric({ label, value, note, icon: Icon }: { label: string; value?: number; note: string; icon: React.ElementType }) {
  return <Card className="min-w-0 gap-0 p-3 sm:p-4"><div className="flex items-center justify-between gap-2"><p className="text-xs font-medium text-muted-foreground">{label}</p><span className="hidden size-7 shrink-0 place-items-center rounded-lg border border-border-subtle bg-bg-sunken text-foreground sm:grid"><Icon className="size-3.5" strokeWidth={1.75} aria-hidden="true" /></span></div><p className="mt-3 break-all text-2xl font-semibold tracking-[-.04em] tabular-nums">{value?.toLocaleString("zh-CN") ?? "—"}</p><p className="mt-1 text-xs leading-5 text-muted-foreground">{note}</p></Card>
}

function UserItem({ item, busy, onEdit, onDetail }: { onDetail: () => void; item: UserRow; busy: boolean; onEdit: () => void }) {
  const name = item.display_name || item.email
  return (
    <TableRow>
      <TableCell className="px-4 py-3.5"><div className="flex min-w-0 items-center gap-3"><span className="grid size-8 shrink-0 place-items-center rounded-full border border-border-default bg-bg-sunken text-xs font-medium" aria-hidden="true">{Array.from(name)[0]?.toUpperCase() || "U"}</span><div className="min-w-0 whitespace-normal"><p className="break-words font-medium leading-5">{name}</p><p className="mt-1 break-all font-mono text-xs leading-4 text-muted-foreground"><bdi>{item.email}</bdi></p></div></div></TableCell>
      <TableCell className="px-4 py-3.5"><Badge variant="outline" className={item.role === "admin" ? "gap-1.5 border-accent-border bg-accent-subtle px-2 text-accent-text" : "px-2 text-muted-foreground"}>{item.role === "admin" && <Shield aria-hidden="true" />}{item.role === "admin" ? "管理员" : "普通用户"}</Badge></TableCell>
      <TableCell className="px-4 py-3.5"><Badge variant="outline" className="gap-1.5 px-2"><span className={`size-1.5 rounded-full ${item.status === 1 ? "bg-success-solid" : "bg-muted-foreground/60"}`} aria-hidden="true" />{item.status === 1 ? "正常" : "已停用"}</Badge></TableCell>
      <TableCell className="px-4 py-3.5"><time dateTime={item.create_time ? new Date(item.create_time * 1000).toISOString() : undefined} className="font-mono text-xs text-muted-foreground">{item.create_time ? new Date(item.create_time * 1000).toLocaleDateString("zh-CN") : "—"}</time></TableCell>
      <TableCell className="px-4 py-3.5"><div className="-ms-2 flex items-center gap-2"><AppTooltip content="用户详情"><Button size="icon-lg" variant="ghost" aria-label={`查看 ${name} 的详情`} onClick={onDetail}><Eye aria-hidden="true" /></Button></AppTooltip><AppTooltip content="编辑用户"><Button size="icon-lg" variant="ghost" disabled={busy} aria-label={`编辑 ${name}`} onClick={onEdit}><Pencil aria-hidden="true" /></Button></AppTooltip></div></TableCell>
    </TableRow>
  )
}
