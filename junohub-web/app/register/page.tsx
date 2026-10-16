import type { Metadata } from "next"
import { AuthPage } from "@/components/auth/auth-page"

export const metadata: Metadata = {
  title: "创建账户 · JunoHub",
}

export default function RegisterPage() {
  return <AuthPage key="register" mode="register" />
}
