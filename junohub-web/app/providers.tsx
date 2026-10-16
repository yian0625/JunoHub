"use client"

import { ThemeProvider } from "next-themes"
import { Toaster } from "@/components/ui/sonner"

export function Providers({ children }: { children: React.ReactNode }) {
  if (typeof window !== "undefined" && typeof window.localStorage === "undefined") {
    return (
      <>
        {children}
        <Toaster position="bottom-right" richColors duration={3500} visibleToasts={3} closeButton />
      </>
    )
  }

  return (
    <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
      {children}
      <Toaster position="bottom-right" richColors duration={3500} visibleToasts={3} closeButton />
    </ThemeProvider>
  )
}
