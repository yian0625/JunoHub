import type { Metadata } from "next"
import { Providers } from "./providers"
import "./globals.css"

export const metadata: Metadata = {
  title: "Juno Hub",
  description: "Juno Hub Gateway Console",
}

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode
}>) {
  return (
    <html lang="zh-CN" className="h-full antialiased" suppressHydrationWarning>
      <body className="min-h-full">
        <a href="#main-content" className="skip-link">跳到主要内容</a>
        <Providers>{children}</Providers>
      </body>
    </html>
  )
}
