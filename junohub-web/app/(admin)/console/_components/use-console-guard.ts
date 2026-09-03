"use client"

import { useAuthStore } from "@/lib/store"

export function useConsoleGuard() {
  const role = useAuthStore(state => state.user?.role)
  return role === "user" || role === "admin"
}
