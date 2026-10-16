"use client"

import Link from "next/link"
import { usePathname } from "next/navigation"
import { useEffect, useState } from "react"
import { Activity, BookOpen, Cable, ChevronUp, KeyRound, LayoutDashboard, LogOut, Network, PackageSearch, ScrollText, Server, Settings, Users, X } from "lucide-react"
import type { LucideIcon } from "lucide-react"
import { cn } from "@/lib/utils"
import { useAuthStore } from "@/lib/store"
import { BrandLogo } from "./brand-logo"
import { ThemeToggle } from "./theme-toggle"
import { SettingsDialog } from "./settings-dialog"
import { Avatar, AvatarFallback, AvatarImage } from "./ui/avatar"
import { Button } from "./ui/button"
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "./ui/dropdown-menu"
import {
  Sidebar as SidebarPrimitive,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarSeparator,
  SidebarTrigger,
  useSidebar,
} from "./ui/sidebar"

type NavItem = { label: string; href: string; icon: LucideIcon }
const gatewayNav: NavItem[] = [
  { label: "网关总览", href: "/console", icon: LayoutDashboard },
  { label: "供应商", href: "/console/providers", icon: Server },
  { label: "模型路由", href: "/console/models", icon: Network },
  { label: "我的密钥", href: "/console/my-keys", icon: KeyRound },
  { label: "请求日志", href: "/console/logs", icon: ScrollText },
]
const personalNav: NavItem[] = [
  { label: "个人总览", href: "/console", icon: LayoutDashboard },
  { label: "可用模型", href: "/console/available-models", icon: Network },
  { label: "我的密钥", href: "/console/my-keys", icon: KeyRound },
  { label: "请求日志", href: "/console/logs", icon: ScrollText },
  { label: "接入指引", href: "/console/guide", icon: BookOpen },
]
const integrationNav: NavItem[] = [
  { label: "客户端配置", href: "/console/connect/providers", icon: Cable },
  { label: "客户端安装", href: "/console/connect/clients", icon: PackageSearch },
]
const adminNav: NavItem[] = [
  { label: "运行监控", href: "/console/monitor", icon: Activity },
  { label: "访问密钥", href: "/console/keys", icon: KeyRound },
  { label: "用户管理", href: "/juno-hub", icon: Users },
  { label: "接入指引", href: "/console/guide", icon: BookOpen },
]

function Brand({ admin }: { admin: boolean }) {
  return <Link href="/console" className="sidebar-brand flex min-h-11 w-fit max-w-full items-center gap-5 rounded-lg px-2 focus-visible:outline-2 focus-visible:outline-ring group-data-[collapsible=icon]:mx-auto group-data-[collapsible=icon]:size-11 group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:px-0" aria-label={`JunoHub ${admin ? "网关总览" : "个人总览"}`}>
    <BrandLogo className="size-8 shrink-0" />
    <span className="text-lg font-serif font-bold italic tracking-wide group-data-[collapsible=icon]:hidden">JunoHub</span>
  </Link>
}

function NavGroup({ label, items, onNavigate }: { label: string; items: NavItem[]; onNavigate: () => void }) {
  const pathname = usePathname()
  return <SidebarGroup className="sidebar-nav-group">
    <SidebarGroupLabel className="sidebar-nav-label group-data-[collapsible=icon]:hidden">{label}</SidebarGroupLabel>
    <SidebarGroupContent>
      <SidebarMenu className="sidebar-nav-menu">
        {items.map(item => {
          const active = pathname === item.href
          return <SidebarMenuItem key={item.href}>
            <SidebarMenuButton
              render={<Link href={item.href} onClick={onNavigate} aria-label={item.label} />}
              isActive={active}
              tooltip={item.label}
              aria-current={active ? "page" : undefined}
              className={cn("sidebar-nav-link group-data-[collapsible=icon]:mx-auto group-data-[collapsible=icon]:justify-center", active && "sidebar-nav-link-active")}
            >
              <item.icon strokeWidth={1.8} aria-hidden="true" />
              <span className="group-data-[collapsible=icon]:hidden">{item.label}</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        })}
      </SidebarMenu>
    </SidebarGroupContent>
  </SidebarGroup>
}

function AccountSummary() {
  const [settingsOpen, setSettingsOpen] = useState(false)
  const { user, logout } = useAuthStore()
  const label = user?.nickname || user?.email || "当前账户"
  return <>
    <DropdownMenu>
      <DropdownMenuTrigger className="sidebar-account flex w-full items-center gap-3 text-start focus-visible:outline-2 focus-visible:outline-ring group-data-[collapsible=icon]:justify-center" aria-label="打开账户菜单">
        <Avatar className="size-9 shrink-0">
          {user?.avatar_url && <AvatarImage src={user.avatar_url} alt="" />}
          <AvatarFallback className="text-[11px] font-semibold">{label.slice(0, 1).toUpperCase()}</AvatarFallback>
        </Avatar>
        <span className="min-w-0 flex-1 group-data-[collapsible=icon]:hidden">
          <span className="block truncate text-[13px] font-medium">{label}</span>
          <span className="block text-[11px] text-muted-foreground">{user?.role === "admin" ? "管理员" : "成员"}</span>
        </span>
        <ChevronUp className="size-4 shrink-0 text-muted-foreground group-data-[collapsible=icon]:hidden" aria-hidden="true" />
      </DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="start" sideOffset={8} className="w-56 rounded-xl p-1.5">
        <DropdownMenuGroup>
          <DropdownMenuLabel className="px-2 py-2"><span className="block truncate text-xs font-semibold">{label}</span>{user?.email && user.email !== label && <span className="mt-1 block truncate font-normal text-muted-foreground">{user.email}</span>}</DropdownMenuLabel>
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <DropdownMenuItem onClick={() => setSettingsOpen(true)}><Settings className="size-4" aria-hidden="true" />设置</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem onClick={logout} variant="destructive"><LogOut className="size-4" aria-hidden="true" />退出登录</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
    {settingsOpen && <SettingsDialog open onOpenChange={setSettingsOpen} />}
  </>
}

export function AppSidebar() {
  const admin = useAuthStore(state => state.user?.role === "admin")
  const { setOpenMobile } = useSidebar()
  const closeMobile = () => setOpenMobile(false)
  return <SidebarPrimitive collapsible="icon" className="border-sidebar-border">
    <SidebarHeader className="sidebar-header h-16 justify-center border-b border-sidebar-border px-3">
      <div className="relative flex w-full items-center justify-start">
        <Brand admin={admin} />
        <Button variant="ghost" size="icon-sm" className="absolute end-0 top-1/2 -translate-y-1/2 lg:hidden" onClick={closeMobile} aria-label="关闭导航"><X aria-hidden="true" /></Button>
      </div>
    </SidebarHeader>
    <SidebarContent>
      <nav aria-label="主要导航" className="sidebar-navigation">
        <NavGroup label={admin ? "网关" : "个人"} items={admin ? gatewayNav : personalNav} onNavigate={closeMobile} />
        <NavGroup label="集成" items={integrationNav} onNavigate={closeMobile} />
        {admin && <NavGroup label="管理" items={adminNav} onNavigate={closeMobile} />}
      </nav>
    </SidebarContent>
    <SidebarFooter className="sidebar-footer">
      <SidebarSeparator className="sidebar-footer-separator" />
      <AccountSummary />
    </SidebarFooter>
  </SidebarPrimitive>
}

export function AppTopbar() {
  const pathname = usePathname()
  const admin = useAuthStore(state => state.user?.role === "admin")
  const titleMap: Record<string, string> = { "/console/monitor": "运行监控", "/juno-hub": "用户管理", "/console/providers": "供应商", "/console/models": "模型路由", "/console/my-keys": "我的密钥", "/console/keys": "访问密钥", "/console/connect": "集成", "/console/connect/providers": "客户端配置", "/console/connect/clients": "客户端安装", "/console/logs": "请求日志", "/console/guide": "接入指引" }
  const title = pathname === "/console/available-models" ? "可用模型" : titleMap[pathname] || (admin ? "网关总览" : "个人总览")
  useEffect(() => { document.title = `${title} · JunoHub` }, [title])
  return <header className="topbar sticky top-0 z-30 h-16">
    <div className="mx-auto flex h-full w-full max-w-[1440px] items-center justify-between gap-4 px-4 sm:px-6 lg:px-8">
      <div className="flex min-w-0 items-center gap-3 sm:-ms-2 lg:-ms-4">
        <SidebarTrigger className="topbar-sidebar-trigger" aria-label="切换侧边栏" />
        <span className="truncate text-[15px] font-semibold">{title}</span>
      </div>
      <div className="flex items-center gap-2"><ThemeToggle /></div>
    </div>
  </header>
}
