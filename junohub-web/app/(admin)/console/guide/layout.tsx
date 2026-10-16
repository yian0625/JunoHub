import type { Metadata } from "next"

export const metadata: Metadata = {
  title: "接入指引 · JunoHub",
}

export default function GuideLayout({children}: {children: React.ReactNode}) {
  return children
}
