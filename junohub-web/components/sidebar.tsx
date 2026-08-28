"use client"

import Link from "next/link"
import { usePathname } from "next/navigation"
import { useEffect, useState } from "react"
import { KeyRound, LayoutDashboard, LogOut, Menu, Network, Orbit, ShieldCheck, Users } from "lucide-react"
import { cn } from "@/lib/utils"
import { useAuthStore } from "@/lib/store"
import { ThemeToggle } from "./theme-toggle"
import { Button } from "./ui/button"
import { Sheet, SheetContent, SheetDescription, SheetFooter, SheetHeader, SheetTitle, SheetTrigger } from "./ui/sheet"

const userNav = [
  { label: "网关总览", href: "/console", icon: LayoutDashboard },
  { label: "供应商", href: "/console#providers", icon: ShieldCheck },
  { label: "模型路由", href: "/console#models", icon: Network },
  { label: "访问密钥", href: "/console#keys", icon: KeyRound },
]

function useActiveLocation() {
  const pathname = usePathname()
  const [hash, setHash] = useState("")
  useEffect(() => {
    const sync = () => setHash(window.location.hash)
    sync()
    window.addEventListener("hashchange", sync)
    return () => window.removeEventListener("hashchange", sync)
  }, [pathname])
  return { pathname, hash }
}

function Brand() {
  return (
    <Link href="/" className="flex min-h-11 items-center gap-3 rounded-lg focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
      <span className="brand-mark size-9 !bg-white !text-[var(--ink-fixed)]" aria-hidden="true"><Orbit className="size-[18px]" strokeWidth={2} /></span>
      <span><b className="block text-sm font-semibold tracking-[-.02em]">Juno Hub</b><span className="mt-0.5 block text-[11px] text-white/45">模型网关</span></span>
    </Link>
  )
}

function Navigation({ onNavigate }: { onNavigate?: () => void }) {
  const { pathname, hash } = useActiveLocation()
  const admin = useAuthStore(state => state.user?.role === "admin")
  const items = admin ? [{ label: "用户管理", href: "/juno-hub", icon: Users }] : userNav
  return (
    <nav aria-label="主要导航" className="space-y-1">
      {items.map(item => {
        const [path, anchor = ""] = item.href.split("#")
        const active = pathname === path && hash === (anchor ? `#${anchor}` : "")
        return (
          <Link key={item.href} href={item.href} onClick={onNavigate} aria-current={active ? "page" : undefined} className={cn("nav-item", active && "nav-item-active")}>
            <item.icon className="size-[18px]" strokeWidth={active ? 2 : 1.5} aria-hidden="true" />
            <span>{item.label}</span>
          </Link>
        )
      })}
    </nav>
  )
}

function AccountSummary() {
  const { user, logout } = useAuthStore()
  const label = user?.nickname || user?.email || "当前账户"
  return (
    <div className="rounded-lg border border-white/8 bg-white/[.035] p-2">
      <div className="flex items-center gap-3 px-2 py-2">
        <span className="grid size-8 shrink-0 place-items-center rounded-md bg-white/10 text-xs font-semibold text-white">{label.slice(0, 1).toUpperCase()}</span>
        <span className="min-w-0 flex-1"><span className="block truncate text-xs font-medium text-white/90">{label}</span><span className="mt-0.5 block text-[11px] text-white/40">{user?.role === "admin" ? "管理员" : "个人工作区"}</span></span>
      </div>
      <Button variant="ghost" size="sm" className="mt-1 w-full justify-start text-white/45 hover:bg-white/[.06] hover:text-white" onClick={logout}><LogOut aria-hidden="true" />退出登录</Button>
    </div>
  )
}

export function Sidebar() {
  return (
    <aside className="admin-sidebar fixed inset-y-0 inset-inline-start-0 z-40 hidden w-56 flex-col px-3 py-5 lg:flex">
      <div className="px-2"><Brand /></div>
      <div className="mt-10 px-3 text-[11px] font-medium text-white/35">工作区</div>
      <div className="mt-2 flex-1"><Navigation /></div>
      <AccountSummary />
    </aside>
  )
}

export function AppTopbar() {
  const pathname = usePathname()
  const [open, setOpen] = useState(false)
  const title = pathname === "/juno-hub" ? "用户管理" : "网关控制台"
  useEffect(() => {
    document.title = `${title} · Juno Hub`
  }, [title])
  return (
    <header className="topbar sticky top-0 z-30 flex min-h-16 items-center justify-between gap-4 px-4 sm:px-6 lg:px-8">
      <div className="flex min-w-0 items-center gap-3">
        <Sheet open={open} onOpenChange={setOpen}>
          <SheetTrigger render={<Button variant="outline" size="icon" className="lg:hidden" aria-label="打开导航" />}><Menu aria-hidden="true" /></SheetTrigger>
          <SheetContent side="left" className="w-[min(88vw,320px)] border-0 bg-[var(--ink-fixed)] p-0 text-white" showCloseButton>
            <SheetHeader className="px-5 pb-2 pt-5"><SheetTitle><Brand /></SheetTitle><SheetDescription className="sr-only">Juno Hub 主要导航</SheetDescription></SheetHeader>
            <div className="flex-1 px-4 py-6"><Navigation onNavigate={() => setOpen(false)} /></div>
            <SheetFooter className="p-4"><AccountSummary /></SheetFooter>
          </SheetContent>
        </Sheet>
        <div className="lg:hidden"><span className="text-sm font-semibold">{title}</span></div>
        <div className="hidden lg:block"><span className="text-sm text-muted-foreground">Juno Hub</span><span className="mx-2 text-border-strong">/</span><span className="text-sm font-medium">{title}</span></div>
      </div>
      <div className="flex items-center gap-2">
        <span className="hidden items-center gap-2 rounded-full bg-success-subtle px-3 py-1.5 text-xs font-medium text-success-text sm:inline-flex"><span className="size-1.5 rounded-full bg-success-solid" aria-hidden="true" />网关运行正常</span>
        <ThemeToggle />
      </div>
    </header>
  )
}
