"use client"

import { useRef, useState } from "react"
import { useRouter } from "next/navigation"
import { ArrowRight, Orbit, RotateCw, ShieldCheck } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { ThemeToggle } from "@/components/theme-toggle"
import { authAPI } from "@/lib/api"
import { useAuthStore } from "@/lib/store"
import { toast } from "sonner"

type Mode = "login" | "register"
type FieldErrors = { email?: string; password?: string; form?: string }

export default function LoginPage() {
  const router = useRouter()
  const saveLogin = useAuthStore(state => state.login)
  const emailRef = useRef<HTMLInputElement>(null)
  const passwordRef = useRef<HTMLInputElement>(null)
  const [mode, setMode] = useState<Mode>("login")
  const [account, setAccount] = useState("")
  const [password, setPassword] = useState("")
  const [displayName, setDisplayName] = useState("")
  const [loading, setLoading] = useState(false)
  const [errors, setErrors] = useState<FieldErrors>({})

  const clearError = (field: keyof FieldErrors) => setErrors(current => ({ ...current, [field]: undefined, form: undefined }))
  const submit = async (event: React.FormEvent) => {
    event.preventDefault()
    const email = account.trim().toLowerCase()
    const nextErrors: FieldErrors = {}
    if (!email) nextErrors.email = "请输入邮箱地址。"
    else if (!/^[^\s@]+@[^\s@]+$/.test(email)) nextErrors.email = "请输入有效的邮箱地址，例如 name@example.com。"
    if (!password) nextErrors.password = "请输入密码。"
    else if (mode === "register" && password.length < 8) nextErrors.password = "请使用至少 8 个字符设置密码。"
    if (Object.keys(nextErrors).length) {
      setErrors(nextErrors)
      requestAnimationFrame(() => (nextErrors.email ? emailRef.current : passwordRef.current)?.focus())
      return
    }
    setLoading(true); setErrors({})
    try {
      const data = mode === "register" ? await authAPI.register({ email, password, display_name: displayName.trim() }) : await authAPI.login({ email, password })
      saveLogin(data.token, data); toast.success(mode === "register" ? "账户已创建" : "登录成功")
      router.replace(data.role === "admin" ? "/juno-hub" : "/console")
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : "无法完成操作，请检查网络后重试。"
      setErrors({ form: message }); toast.error(message)
    } finally { setLoading(false) }
  }

  const changeMode = (next: Mode) => { setMode(next); setPassword(""); setErrors({}) }

  return (
    <main id="main-content" tabIndex={-1} className="relative flex min-h-[100dvh] items-center justify-center px-5 py-20 focus:outline-none sm:px-8">
      <div className="absolute inset-inline-end-5 top-5 sm:inset-inline-end-8 sm:top-8"><ThemeToggle /></div>
      <section className="w-full max-w-[400px]" aria-labelledby="auth-title">
        <div className="mb-12 flex items-center gap-3"><span className="brand-mark size-9"><Orbit className="size-[18px]" aria-hidden="true" /></span><span className="text-sm font-semibold tracking-[-.02em]">Juno Hub</span></div>
        <header><h1 id="auth-title" className="text-2xl font-semibold tracking-[-.035em]">{mode === "login" ? "登录" : "创建账户"}</h1><p className="mt-2 text-sm leading-6 text-muted-foreground">{mode === "login" ? "使用 Juno Hub 账户继续。" : "创建账户后即可配置模型网关。"}</p></header>

        <div className="mt-8 flex gap-6 border-b border-border-subtle" role="group" aria-label="账户操作">
          {(["login", "register"] as Mode[]).map(item => <button key={item} type="button" aria-pressed={mode === item} onClick={() => changeMode(item)} className="relative min-h-11 px-0 text-sm font-medium text-muted-foreground transition-colors duration-150 aria-pressed:text-foreground aria-pressed:after:absolute aria-pressed:after:inset-x-0 aria-pressed:after:-bottom-px aria-pressed:after:h-0.5 aria-pressed:after:bg-foreground">{item === "login" ? "登录" : "注册"}</button>)}
        </div>

        <form noValidate onSubmit={submit} className="mt-7 space-y-5" aria-busy={loading}>
              {mode === "register" && <Field id="display-name" label="显示名称" hint="可选"><Input id="display-name" name="name" autoComplete="name" value={displayName} onChange={event => setDisplayName(event.target.value)} placeholder="例如：小林" /></Field>}
              <Field id="account" label="邮箱" error={errors.email}><Input ref={emailRef} id="account" name="email" type="email" inputMode="email" autoComplete="username" spellCheck={false} value={account} onChange={event => { setAccount(event.target.value); clearError("email") }} placeholder="name@example.com" aria-invalid={Boolean(errors.email)} aria-describedby={errors.email ? "account-error" : undefined} /></Field>
              <Field id="password" label="密码" hint={mode === "register" ? "至少 8 个字符" : undefined} error={errors.password}><Input ref={passwordRef} id="password" name="password" type="password" autoComplete={mode === "register" ? "new-password" : "current-password"} value={password} onChange={event => { setPassword(event.target.value); clearError("password") }} placeholder="输入密码" aria-invalid={Boolean(errors.password)} aria-describedby={errors.password ? "password-error" : undefined} /></Field>
              {errors.form && <p role="alert" className="rounded-xl bg-danger-subtle px-3.5 py-3 text-sm leading-5 text-danger-text">{errors.form} 请确认信息后重试。</p>}
          <Button type="submit" size="lg" className="mt-2 w-full" disabled={loading}>{loading && <RotateCw className="animate-spin" aria-hidden="true" />}<span>{mode === "register" ? "创建账户" : "登录"}</span>{!loading && <ArrowRight className="icon-directional" aria-hidden="true" />}</Button>
        </form>
        <p className="mt-7 flex items-center gap-2 text-xs leading-5 text-muted-foreground"><ShieldCheck className="size-3.5" strokeWidth={1.5} aria-hidden="true" />模型凭证仅保存在你的私有部署中</p>
      </section>
    </main>
  )
}

function Field({ id, label, hint, error, children }: { id: string; label: string; hint?: string; error?: string; children: React.ReactNode }) {
  return <div className="space-y-2"><div className="flex items-center justify-between gap-3"><Label htmlFor={id} className="text-sm font-medium">{label}</Label>{hint && <span className="text-xs text-muted-foreground">{hint}</span>}</div>{children}{error && <p id={`${id}-error`} className="text-sm leading-5 text-danger-text">{error}</p>}</div>
}
