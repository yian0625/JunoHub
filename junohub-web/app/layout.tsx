import type { Metadata } from "next"
import { Geist, Geist_Mono } from "next/font/google"
import { Providers } from "./providers"
import "./globals.css"

const geist = Geist({
  subsets: ["latin"],
  variable: "--font-geist",
  display: "swap",
})

const geistMono = Geist_Mono({
  subsets: ["latin"],
  variable: "--font-geist-mono",
  display: "swap",
})

export const metadata: Metadata = {
  title: "JunoHub",
  description: "JunoHub Gateway Console",
}

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode
}>) {
  return (
    <html lang="zh-CN" className={`h-full antialiased ${geist.variable} ${geistMono.variable}`} suppressHydrationWarning>
      <body className="min-h-full">
        <a href="#main-content" className="skip-link">跳到主要内容</a>
        <Providers>{children}</Providers>
      </body>
    </html>
  )
}
