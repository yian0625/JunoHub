"use client"

import { useEffect, useState } from "react"
import { useRouter } from "next/navigation"
import { useAuthStore } from "@/lib/store"
import { accountAPI } from "@/lib/api"

export function useAuth() {
  const router = useRouter()
  const { token, user, init, logout, isReady, updateUser } = useAuthStore()
  const [verifiedToken, setVerifiedToken] = useState<string | null>(null)
  const [error, setError] = useState("")
  const [revision, setRevision] = useState(0)

  useEffect(() => {
    init()
  }, [init])

  useEffect(() => {
    if (isReady && !token) {
      router.replace("/login")
    }
  }, [isReady, router, token])

  useEffect(() => {
    if (!isReady || !token) return
    let active = true
    accountAPI.profile().then(profile => {
      if (!active) return
      updateUser(profile); setVerifiedToken(token); setError("")
    }).catch(error => { if (active) setError(error instanceof Error ? error.message : "无法确认账户身份") })
    return () => { active = false }
  }, [isReady, token, updateUser, revision])

  return { token, user, logout, isReady: isReady && (!token || verifiedToken === token), error, retry: () => { setError(""); setRevision(value => value + 1) } }
}
