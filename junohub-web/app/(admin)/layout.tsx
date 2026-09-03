"use client"

import { useState } from "react"
import { useAuth } from "@/hooks/use-auth"
import { AppTopbar, Sidebar } from "@/components/sidebar"
import { cn } from "@/lib/utils"

export default function AdminLayout({
  children,
}: {
  children: React.ReactNode
}) {
  const { isReady, token } = useAuth()
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false)

  if (!isReady || !token) {
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
    <div className="admin-shell min-h-screen pt-4">
      <AppTopbar sidebarCollapsed={sidebarCollapsed} onSidebarToggle={() => setSidebarCollapsed(value => !value)} />
      <Sidebar collapsed={sidebarCollapsed} />
      <div className={cn("admin-content transition-[padding] duration-200", sidebarCollapsed ? "lg:ps-28" : "lg:ps-72")}>
        <main id="main-content" tabIndex={-1} className="admin-main max-w-[1440px] px-4 py-8 focus:outline-none sm:px-6 lg:pe-10 lg:ps-6 lg:py-10">{children}</main>
      </div>
    </div>
  )
}
