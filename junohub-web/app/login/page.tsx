"use client"

import { useRef, useState } from "react"
import { useRouter } from "next/navigation"
import { ArrowRight, KeyRound, Network, RotateCw, ShieldCheck } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { BrandLogo } from "@/components/brand-logo"
import { authAPI } from "@/lib/api"
import { useAuthStore } from "@/lib/store"
import { cn } from "@/lib/utils"
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
    <main id="main-content" tabIndex={-1} className="grid min-h-[100dvh] place-items-center px-5 py-10 focus:outline-none sm:px-8">
      <div className="grid w-full max-w-[1024px] overflow-hidden rounded-[2rem] border border-[var(--glass-border)] bg-[var(--glass-bg)] shadow-[var(--glass-highlight),var(--shadow-surface)] backdrop-blur-2xl backdrop-saturate-150 md:grid-cols-[minmax(0,1.08fr)_minmax(0,.92fr)]">
        <div className="flex flex-col gap-12 p-9 sm:p-12">
          <div className="flex items-center gap-3">
            <BrandLogo className="size-11" />
            <b className="block text-2xl font-serif font-bold">JunoHub</b>
          </div>
          <div className="hidden md:block">
            <h1 className="text-[2.25rem] font-semibold leading-[1.15] tracking-[-.03em]">统一你的模型接入</h1>
            <p className="mt-6 text-[15px] leading-8 text-muted-foreground">
              <span className="block">上游服务、模型路由与访问密钥，集中在一个兼容入口</span>
              <span className="block">所有请求，一条通路</span>
            </p>
            <ul className="mt-10 space-y-5">
              {[
                { icon: Network, title: "模型路由", note: "上游模型映射为稳定别名" },
                { icon: KeyRound, title: "独立密钥", note: "按应用签发，随时单独撤销" },
                { icon: ShieldCheck, title: "私有部署", note: "凭证只存在你自己的服务器上" },
              ].map(item => (
                <li key={item.title} className="flex items-start gap-4">
                  <span className="mt-0.5 grid size-10 shrink-0 place-items-center rounded-2xl border border-border-subtle bg-bg-surface/60 text-accent-text"><item.icon className="size-[18px]" strokeWidth={1.75} aria-hidden="true" /></span>
                  <span><span className="block text-[15px] font-medium">{item.title}</span><span className="mt-1 block text-[13px] leading-5 text-muted-foreground">{item.note}</span></span>
                </li>
              ))}
            </ul>
          </div>
        </div>

        <div className="border-t border-[var(--glass-border)] p-9 sm:p-12 md:border-t-0 md:border-l" aria-labelledby="auth-title">
            <header>
              <h1 id="auth-title" className="text-2xl font-semibold leading-tight tracking-[-.03em]">{mode === "login" ? "登录" : "创建账户"}</h1>
              <p className="mt-2.5 text-sm leading-6 text-muted-foreground">{mode === "login" ? "使用 JunoHub 账户继续。" : "创建账户后即可配置模型网关。"}</p>
            </header>

            <div className="mt-7 grid grid-cols-2 gap-1 rounded-full border border-[var(--glass-border)] bg-[var(--glass-bg-subtle)] p-1" role="group" aria-label="账户操作">
              {(["login", "register"] as Mode[]).map(item => (
                <button key={item} type="button" aria-pressed={mode === item} onClick={() => changeMode(item)} className={cn("min-h-10 rounded-full text-sm transition-all duration-200", mode === item ? "bg-bg-surface font-semibold text-foreground shadow-[var(--glass-highlight),var(--shadow-surface)]" : "font-medium text-muted-foreground hover:text-foreground")}>{item === "login" ? "登录" : "注册"}</button>
              ))}
            </div>

            <form noValidate onSubmit={submit} className="mt-7 space-y-5" aria-busy={loading}>
              {mode === "register" && <Field id="display-name" label="显示名称" hint="可选"><Input id="display-name" name="name" autoComplete="name" value={displayName} onChange={event => setDisplayName(event.target.value)} placeholder="例如：小林" className="h-12" /></Field>}
              <Field id="account" label="邮箱" error={errors.email}><Input ref={emailRef} id="account" name="email" type="email" inputMode="email" autoComplete="username" spellCheck={false} value={account} onChange={event => { setAccount(event.target.value); clearError("email") }} placeholder="name@example.com" className="h-12" aria-invalid={Boolean(errors.email)} aria-describedby={errors.email ? "account-error" : undefined} /></Field>
              <Field id="password" label="密码" hint={mode === "register" ? "至少 8 个字符" : undefined} error={errors.password}><Input ref={passwordRef} id="password" name="password" type="password" autoComplete={mode === "register" ? "new-password" : "current-password"} value={password} onChange={event => { setPassword(event.target.value); clearError("password") }} placeholder="输入密码" className="h-12" aria-invalid={Boolean(errors.password)} aria-describedby={errors.password ? "password-error" : undefined} /></Field>
              {errors.form && <p role="alert" className="rounded-xl bg-danger-subtle px-3.5 py-3 text-sm leading-5 text-danger-text">{errors.form} 请确认信息后重试。</p>}
              <Button type="submit" size="lg" className="h-12 w-full" disabled={loading}>{loading && <RotateCw className="animate-spin" aria-hidden="true" />}<span>{mode === "register" ? "创建账户" : "登录"}</span>{!loading && <ArrowRight className="icon-directional" aria-hidden="true" />}</Button>
            </form>
            <p className="mt-6 flex items-center justify-center gap-2 text-xs leading-5 text-muted-foreground"><ShieldCheck className="size-3.5" strokeWidth={1.5} aria-hidden="true" />模型凭证仅保存在你的私有部署中</p>
        </div>
      </div>
    </main>
  )
}

function Field({ id, label, hint, error, children }: { id: string; label: string; hint?: string; error?: string; children: React.ReactNode }) {
  return <div className="space-y-2"><div className="flex items-center justify-between gap-3"><Label htmlFor={id} className="text-sm font-medium">{label}</Label>{hint && <span className="text-xs text-muted-foreground">{hint}</span>}</div>{children}{error && <p id={`${id}-error`} className="text-sm leading-5 text-danger-text">{error}</p>}</div>
}
