"use client"

import { useEffect, useState } from "react"
import { Check, Copy, TriangleAlert } from "lucide-react"
import { Button } from "@/components/ui/button"

const oauthCallbackMessage = "junohub:oauth-callback"

export default function OAuthCallbackClient({ hasCode, error = "" }: { hasCode: boolean; error?: string }) {
  const [copied, setCopied] = useState(false)
  const [copyError, setCopyError] = useState("")

  useEffect(() => {
    if (!hasCode || !window.opener) return
    window.opener.postMessage({ type: oauthCallbackMessage, callbackUrl: window.location.href }, "*")
  }, [hasCode])

  const copyCallback = async () => {
    setCopyError("")
    try {
      await navigator.clipboard.writeText(window.location.href)
      setCopied(true)
    } catch {
      setCopyError("复制失败，请手动复制浏览器地址栏中的完整地址。")
    }
  }

  return (
    <main id="main-content" className="grid min-h-dvh place-items-center px-4 py-10">
      <section className="glass-popup w-full max-w-md rounded-2xl p-6 text-center sm:p-8" aria-live="polite">
        <span className={hasCode ? "mx-auto grid size-12 place-items-center rounded-2xl bg-success-subtle text-success-text" : "mx-auto grid size-12 place-items-center rounded-2xl bg-danger-subtle text-danger-text"}>
          {hasCode ? <Check className="size-6" strokeWidth={2} aria-hidden="true" /> : <TriangleAlert className="size-6" strokeWidth={1.75} aria-hidden="true" />}
        </span>
        <h1 className="mt-5 text-xl font-semibold">{hasCode ? "已收到授权结果" : "没有收到授权码"}</h1>
        <p className="mt-2 text-sm leading-6 text-muted-foreground">
          {hasCode ? "请复制回调地址，返回发起授权的 JunoHub 窗口粘贴并提交。提交成功后才会保存账户。" : error || "请返回 JunoHub，重新生成授权链接后再试。"}
        </p>
        {hasCode && <Button type="button" variant="outline" className="mt-6 w-full" onClick={copyCallback}><Copy aria-hidden="true" />{copied ? "已复制回调地址" : "复制回调地址"}</Button>}
        {copyError && <p role="alert" className="mt-3 text-sm text-danger-text">{copyError}</p>}
      </section>
    </main>
  )
}
