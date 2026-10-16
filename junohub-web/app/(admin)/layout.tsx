"use client"

import { useEffect, useState } from "react"
import { usePathname, useRouter } from "next/navigation"
import { Button } from "@/components/ui/button"
import { useAuth } from "@/hooks/use-auth"
import { AppSidebar, AppTopbar } from "@/components/sidebar"
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar"
import type { CSSProperties } from "react"

export default function AdminLayout({
  children,
}: {
  children: React.ReactNode
}) {
  const { isReady, token, user, error, retry } = useAuth()
  const pathname = usePathname()
  const router = useRouter()
  const restricted = user?.role !== "admin" && ["/console/monitor", "/console/providers", "/console/models", "/console/keys", "/juno-hub"].some(path => pathname === path || pathname.startsWith(`${path}/`))
  useEffect(() => {
    if (isReady && token && restricted) router.replace(pathname.startsWith("/console/models") ? "/console/available-models" : pathname.startsWith("/console/keys") ? "/console/my-keys" : "/console")
  }, [isReady, token, restricted, pathname, router])
  const [sidebarOpen, setSidebarOpen] = useState(() =>
    typeof document === "undefined" ||
    document.cookie.split("; ").find(part => part.startsWith("sidebar_state="))?.split("=")[1] !== "false"
  )

  if (error) return <div role="alert" className="flex min-h-screen flex-col items-center justify-center gap-4 px-6"><p className="text-sm">{error}</p><Button variant="outline" onClick={retry}>重新加载</Button></div>
  if (!isReady || !token || restricted) {
    return (
      <div className="flex min-h-screen items-center justify-center" role="status" aria-label="正在加载控制台">
        <div className="flex items-center gap-3 text-muted-foreground">
          <div className="size-5 animate-spin rounded-full border-2 border-muted border-t-primary" aria-hidden="true" />
          <span className="text-sm">正在打开控制台…</span>
        </div>
      </div>
    )
  }

  return (
    <SidebarProvider open={sidebarOpen} onOpenChange={setSidebarOpen} className="admin-shell min-h-screen" style={{ "--sidebar-width": "15rem", "--sidebar-width-icon": "5rem" } as CSSProperties}>
      <AppSidebar />
      <SidebarInset id="main-content" tabIndex={-1} className="admin-content min-w-0 bg-bg-page focus:outline-none">
        <AppTopbar />
        <div className="admin-main mx-auto w-full max-w-[1440px] px-4 py-8 sm:px-6 lg:px-8 lg:py-10">{children}</div>
      </SidebarInset>
    </SidebarProvider>
  )
}
