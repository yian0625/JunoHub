"use client"

import { useEffect } from "react"
import { useRouter } from "next/navigation"
import { useAuthStore } from "@/lib/store"

export function useAuth() {
  const router = useRouter()
  const { token, user, init, logout, isReady } = useAuthStore()

  useEffect(() => {
    init()
  }, [init])

  useEffect(() => {
    if (isReady && !token) {
      router.replace("/login")
    }
  }, [isReady, router, token])

  return { token, user, logout, isReady }
}
