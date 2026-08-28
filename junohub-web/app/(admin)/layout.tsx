"use client"

import { useAuth } from "@/hooks/use-auth"
import { AppTopbar, Sidebar } from "@/components/sidebar"

export default function AdminLayout({
  children,
}: {
  children: React.ReactNode
}) {
  const { isReady, token } = useAuth()

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
    <div className="admin-shell min-h-screen">
      <Sidebar />
      <div className="admin-content lg:ps-56">
        <AppTopbar />
        <main id="main-content" tabIndex={-1} className="admin-main mx-auto max-w-[1380px] px-4 py-8 focus:outline-none sm:px-6 lg:px-10 lg:py-12">{children}</main>
      </div>
    </div>
  )
}
