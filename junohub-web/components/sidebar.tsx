"use client"

import Link from "next/link"
import { usePathname } from "next/navigation"
import { useEffect, useState } from "react"
import { ChevronUp, KeyRound, LayoutDashboard, LogOut, Menu, Network, PanelLeftClose, PanelLeftOpen, ScrollText, Server, Settings, Users } from "lucide-react"
import { cn } from "@/lib/utils"
import { useAuthStore } from "@/lib/store"
import { BrandLogo } from "./brand-logo"
import { ThemeToggle } from "./theme-toggle"
import { SettingsDialog } from "./settings-dialog"
import { Avatar, AvatarFallback, AvatarImage } from "./ui/avatar"
import { Button } from "./ui/button"
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "./ui/dropdown-menu"
import { Sheet, SheetContent, SheetDescription, SheetFooter, SheetHeader, SheetTitle, SheetTrigger } from "./ui/sheet"

const userNav = [
  { label: "网关总览", href: "/console", icon: LayoutDashboard },
  { label: "供应商", href: "/console/providers", icon: Server },
  { label: "模型路由", href: "/console/models", icon: Network },
  { label: "访问密钥", href: "/console/keys", icon: KeyRound },
  { label: "请求日志", href: "/console/logs", icon: ScrollText },
]

const adminNav = [
  ...userNav,
  { label: "用户管理", href: "/juno-hub", icon: Users },
]

function AccountAvatar({ label, src, className }: { label: string; src?: string; className?: string }) {
  return (
    <Avatar className={cn("size-8", className)}>
      {src && <AvatarImage src={src} alt="" />}
      <AvatarFallback className="text-[11px] font-semibold text-foreground">{label.slice(0, 1).toUpperCase()}</AvatarFallback>
    </Avatar>
  )
}

function Brand({ collapsed = false }: { collapsed?: boolean }) {
  return (
    <div className="flex min-h-11 items-center justify-center gap-3" aria-label="JunoHub">
      <BrandLogo className="size-9 shrink-0" />
      {!collapsed && <b className="block text-lg font-serif font-bold">JunoHub</b>}
    </div>
  )
}

function Navigation({ onNavigate, collapsed = false }: { onNavigate?: () => void; collapsed?: boolean }) {
  const pathname = usePathname()
  const admin = useAuthStore(state => state.user?.role === "admin")
  const items = admin ? adminNav : userNav
  return (
    <nav aria-label="主要导航" className="space-y-1">
      {items.map(item => {
        const active = pathname === item.href
        return (
          <Link key={item.href} href={item.href} onClick={onNavigate} aria-current={active ? "page" : undefined} aria-label={collapsed ? item.label : undefined} title={collapsed ? item.label : undefined} className={cn("nav-item", collapsed && "justify-center px-0", active && "nav-item-active")}>
            <item.icon className="nav-item-icon size-[18px] shrink-0" strokeWidth={1.75} aria-hidden="true" />
            <span className={collapsed ? "sr-only" : undefined}>{item.label}</span>
          </Link>
        )
      })}
    </nav>
  )
}

function AccountSummary({ collapsed = false }: { collapsed?: boolean }) {
  const [settingsOpen, setSettingsOpen] = useState(false)
  const { user, logout } = useAuthStore()
  const label = user?.nickname || user?.email || "当前账户"
  return (
    <>
      <div className="border-t border-[var(--glass-border-subtle)] pt-3">
      <DropdownMenu>
        <DropdownMenuTrigger className={cn("flex min-h-11 w-full items-center gap-3 rounded-xl px-3 py-1.5 text-start transition-colors hover:bg-[var(--glass-bg-subtle)] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring", collapsed && "justify-center px-0")} aria-label="打开账户菜单" title={collapsed ? label : undefined}>
          <AccountAvatar label={label} src={user?.avatar_url} />
          {!collapsed && <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-foreground">{label}</span>}
          {!collapsed && <ChevronUp className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />}
        </DropdownMenuTrigger>
        <DropdownMenuContent side="top" align="start" sideOffset={8} className="glass-popup w-52 rounded-xl p-1.5">
          <DropdownMenuGroup>
            <DropdownMenuLabel className="px-2 py-2"><span className="block truncate text-xs font-semibold text-foreground">{label}</span>{user?.email && user.email !== label && <span className="mt-1 block truncate font-normal text-muted-foreground">{user.email}</span>}</DropdownMenuLabel>
          </DropdownMenuGroup>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={() => setSettingsOpen(true)} className="gap-2.5 px-2.5 py-2"><Settings className="size-4" aria-hidden="true" />设置</DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={logout} variant="destructive" className="gap-2.5 px-2.5 py-2"><LogOut className="size-4" aria-hidden="true" />退出登录</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      </div>
      {settingsOpen && <SettingsDialog open onOpenChange={setSettingsOpen} />}
    </>
  )
}

export function Sidebar({ collapsed }: { collapsed: boolean }) {
  return (
    <aside className={cn("admin-sidebar fixed inset-y-4 start-4 z-40 hidden flex-col p-4 transition-[width] duration-200 lg:flex", collapsed ? "w-20" : "w-64")}>
      <div className={cn("border-b border-[var(--glass-border-subtle)] pb-5 pt-1", collapsed ? "px-0" : "px-2")}>
        <Brand collapsed={collapsed} />
      </div>
      <div className="mt-5 min-h-0 flex-1 overflow-y-auto overflow-x-hidden">
        <Navigation collapsed={collapsed} />
      </div>
      <div className="mt-4"><AccountSummary collapsed={collapsed} /></div>
    </aside>
  )
}

export function AppTopbar({ sidebarCollapsed = false, onSidebarToggle }: { sidebarCollapsed?: boolean; onSidebarToggle: () => void }) {
  const pathname = usePathname()
  const [open, setOpen] = useState(false)
  const titleMap: Record<string, string> = { "/juno-hub": "用户管理", "/console/providers": "供应商", "/console/models": "模型路由", "/console/keys": "访问密钥", "/console/logs": "请求日志" }
  const title = titleMap[pathname] || "网关总览"
  useEffect(() => {
    document.title = `${title} · JunoHub`
  }, [title])
  return (
    <header className={cn("topbar sticky top-4 z-30 mx-4 flex min-h-14 items-center justify-between gap-4 px-4 transition-[margin] duration-200 sm:px-5 lg:me-4 lg:px-6", sidebarCollapsed ? "lg:ms-28" : "lg:ms-72")}>
      <div className="flex min-w-0 items-center gap-3">
        <Sheet open={open} onOpenChange={setOpen}>
          <SheetTrigger render={<Button variant="outline" size="icon" className="lg:hidden" aria-label="打开导航" />}><Menu aria-hidden="true" /></SheetTrigger>
          <SheetContent side="left" className="w-[min(88vw,320px)] border-0 bg-bg-surface p-0" showCloseButton>
            <SheetHeader className="border-b border-[var(--glass-border-subtle)] px-5 pb-4 pt-5"><SheetTitle><Brand /></SheetTitle><SheetDescription className="sr-only">JunoHub 主要导航</SheetDescription></SheetHeader>
            <div className="flex-1 px-3 py-4"><Navigation onNavigate={() => setOpen(false)} /></div>
            <SheetFooter className="p-3"><AccountSummary /></SheetFooter>
          </SheetContent>
        </Sheet>
        <Button variant="ghost" size="icon" onClick={onSidebarToggle} className="hidden size-9 rounded-lg text-muted-foreground hover:bg-[var(--glass-bg-subtle)] hover:text-foreground lg:inline-flex" aria-label={sidebarCollapsed ? "展开侧边栏" : "折叠侧边栏"} title={sidebarCollapsed ? "展开侧边栏" : "折叠侧边栏"}>
          {sidebarCollapsed ? <PanelLeftOpen aria-hidden="true" /> : <PanelLeftClose aria-hidden="true" />}
        </Button>
        <div className="lg:hidden"><span className="text-sm font-semibold">{title}</span></div>
        <div className="hidden lg:block"><span className="text-sm text-muted-foreground">JunoHub</span><span className="mx-2 text-border-strong">/</span><span className="text-sm font-semibold text-foreground">{title}</span></div>
      </div>
      <div className="flex items-center gap-2">
        <ThemeToggle />
      </div>
    </header>
  )
}
