"use client"

import { useEffect, useRef, useState, type ReactNode } from "react"
import Link from "next/link"
import { useRouter } from "next/navigation"
import { ArrowRight, Check, Eye, EyeOff, KeyRound, Layers, Network, RotateCw, ScrollText, ShieldCheck } from "lucide-react"
import { toast } from "sonner"
import { BrandLogo } from "@/components/brand-logo"
import { Button } from "@/components/ui/button"
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field"
import { Input } from "@/components/coss-ui/input"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/coss-ui/input-group"
import { OTPField, OTPFieldInput } from "@/components/ui/otp-field"
import { APIError, authAPI } from "@/lib/api"
import { useAuthStore } from "@/lib/store"
import { cn } from "@/lib/utils"

type Mode = "login" | "register"
type FieldErrors = { email?: string; password?: string; verification_code?: string; form?: string }
type InputField = Exclude<keyof FieldErrors, "form">

const emailPattern = /^[^\s@]+@[^\s@]+$/
const OTP_LENGTH = 6
const OTP_SLOT_KEYS = Array.from({ length: OTP_LENGTH }, (_, i) => `otp-slot-${i}`)
const authLinkClassName = "rounded-sm font-medium text-accent-text underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-offset-4 focus-visible:outline-ring"

export function AuthPage({ mode }: { mode: Mode }) {
  const registering = mode === "register"
  const router = useRouter()
  const saveLogin = useAuthStore(state => state.login)
  const emailRef = useRef<HTMLInputElement>(null)
  const passwordRef = useRef<HTMLInputElement>(null)
  const codeRef = useRef<HTMLInputElement>(null)
  const requestPending = useRef(false)
  const [email, setEmail] = useState("")
  const [password, setPassword] = useState("")
  const [showPassword, setShowPassword] = useState(false)
  const [displayName, setDisplayName] = useState("")
  const [verificationCode, setVerificationCode] = useState("")
  const [loading, setLoading] = useState(false)
  const [sendingCode, setSendingCode] = useState(false)
  const [errors, setErrors] = useState<FieldErrors>({})
  const [sentEmail, setSentEmail] = useState("")
  const [sendAfter, setSendAfter] = useState<Record<string, number>>({})
  const [now, setNow] = useState(() => Date.now())
  const normalizedEmail = email.trim().toLowerCase()
  const resendAt = sendAfter[normalizedEmail] ?? 0
  const secondsRemaining = Math.max(0, Math.ceil((resendAt - now) / 1000))
  const busy = loading || sendingCode

  useEffect(() => {
    if (resendAt <= Date.now()) return
    const timer = window.setInterval(() => {
      const timestamp = Date.now()
      setNow(timestamp)
      if (timestamp >= resendAt) window.clearInterval(timer)
    }, 1000)
    return () => window.clearInterval(timer)
  }, [resendAt])

  function focusField(field: InputField) {
    requestAnimationFrame(() => {
      const input = field === "email" ? emailRef.current : field === "password" ? passwordRef.current : codeRef.current
      input?.focus()
    })
  }

  function showError(cause: unknown) {
    const message = cause instanceof Error ? cause.message : "无法完成操作，请检查网络后重试。"
    if (cause instanceof APIError && cause.field) {
      setErrors({ [cause.field]: message })
      focusField(cause.field)
    } else {
      setErrors({ form: message })
    }
  }

  function clearError(field: keyof FieldErrors) {
    setErrors(current => ({ ...current, [field]: undefined, form: undefined }))
  }

  async function sendCode() {
    if (requestPending.current || secondsRemaining > 0) return
    if (!emailPattern.test(normalizedEmail)) {
      setErrors({ email: normalizedEmail ? "请输入有效的邮箱地址，例如 name@example.com。" : "请先填写邮箱地址。" })
      focusField("email")
      return
    }
    requestPending.current = true
    setSendingCode(true)
    setErrors({})
    try {
      const result = await authAPI.sendRegistrationCode(normalizedEmail)
      const timestamp = Date.now()
      setSendAfter(current => ({ ...current, [normalizedEmail]: timestamp + result.retry_after * 1000 }))
      setNow(timestamp)
      setSentEmail(normalizedEmail)
      setVerificationCode("")
      focusField("verification_code")
    } catch (cause) {
      showError(cause)
    } finally {
      requestPending.current = false
      setSendingCode(false)
    }
  }

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (requestPending.current) return
    const nextErrors: FieldErrors = {}
    if (!normalizedEmail) nextErrors.email = "请输入邮箱地址。"
    else if (!emailPattern.test(normalizedEmail)) nextErrors.email = "请输入有效的邮箱地址，例如 name@example.com。"
    if (registering && !/^\d{6}$/.test(verificationCode)) nextErrors.verification_code = "请输入邮件中的 6 位验证码。"
    if (!password) nextErrors.password = registering ? "请设置登录密码。" : "请输入密码。"
    else if (registering && password.length < 8) nextErrors.password = "密码至少需要 8 个字符。"
    if (Object.keys(nextErrors).length) {
      setErrors(nextErrors)
      focusField(nextErrors.email ? "email" : nextErrors.verification_code ? "verification_code" : "password")
      return
    }
    requestPending.current = true
    setLoading(true)
    setErrors({})
    try {
      const data = registering
        ? await authAPI.register({ email: normalizedEmail, password, verification_code: verificationCode, display_name: displayName.trim() })
        : await authAPI.login({ email: normalizedEmail, password })
      saveLogin(data.token, data)
      toast.success(registering ? "账户已创建" : "登录成功")
      router.replace(data.role === "admin" ? "/juno-hub" : "/console")
    } catch (cause) {
      showError(cause)
    } finally {
      requestPending.current = false
      setLoading(false)
    }
  }

  const features = registering ? [
    { icon: Layers, title: "可用模型", note: "浏览模型，选择适合你的接口" },
    { icon: KeyRound, title: "我的密钥", note: "为你的应用创建独立访问密钥" },
    { icon: ScrollText, title: "请求记录", note: "查看个人请求与 Token 用量" },
  ] : [
    { icon: Network, title: "模型路由", note: "上游模型映射为稳定别名" },
    { icon: KeyRound, title: "独立密钥", note: "按应用签发，随时单独撤销" },
    { icon: ShieldCheck, title: "私有部署", note: "凭证只存在你自己的服务器上" },
  ]

  return (
    <main id="main-content" tabIndex={-1} className="grid min-h-[100dvh] place-items-center px-4 py-6 focus:outline-none sm:px-8 sm:py-10">
      <div className="auth-shell grid w-full max-w-[480px] overflow-hidden rounded-[20px] border border-border-default bg-bg-surface lg:max-w-[1024px] lg:grid-cols-[minmax(0,.9fr)_minmax(0,1.1fr)]">
        <aside className="flex flex-col bg-bg-chrome px-6 py-6 sm:px-10 lg:p-12" aria-label="JunoHub 简介">
          <div className="flex items-center gap-5">
            <BrandLogo className="size-10 rounded-lg" />
            <b className="block text-2xl font-serif font-bold italic tracking-wide">JunoHub</b>
          </div>
          <div className="hidden flex-1 flex-col justify-center py-12 lg:flex">
            <p className="whitespace-pre-line text-[2rem] font-semibold leading-[1.4] tracking-[-.03em]">{registering ? "连接模型，\n从这里开始。" : "统一你的\n模型接入。"}</p>
            <p className="mt-4 text-sm leading-7 text-muted-foreground">
              {registering ? "管理你的模型、访问密钥与每一次调用。" : "上游服务、模型路由与访问密钥，集中在一个兼容入口。"}
            </p>
            <ul className="mt-8 space-y-5">
              {features.map(item => (
                <li key={item.title} className="flex items-start gap-3">
                  <span className="mt-0.5 grid size-8 shrink-0 place-items-center rounded-lg bg-bg-surface text-muted-foreground">
                    <item.icon className="size-4" strokeWidth={1.5} aria-hidden="true" />
                  </span>
                  <span>
                    <span className="block text-sm font-medium">{item.title}</span>
                    <span className="mt-1 block text-[13px] leading-5 text-muted-foreground">{item.note}</span>
                  </span>
                </li>
              ))}
            </ul>
          </div>
        </aside>

        <section className="flex min-w-0 flex-col justify-center border-t border-border-default px-6 py-8 sm:px-10 sm:py-10 lg:border-s lg:border-t-0 lg:p-12" aria-labelledby="auth-title">
          <div className="mx-auto w-full max-w-[384px]">
          <header>
            <h1 id="auth-title" className="text-2xl font-semibold leading-tight tracking-[-.03em]">{registering ? "创建账户" : "登录"}</h1>
            <p className="mt-2 text-sm leading-6 text-muted-foreground">{registering ? "用邮箱注册，开启你的模型工作台。" : "使用 JunoHub 账户继续。"}</p>
          </header>

          <form noValidate onSubmit={submit} className="auth-form mt-8 space-y-5" aria-busy={busy}>
            <AuthField id="account" label="邮箱" error={errors.email}>
              <Input ref={emailRef} id="account" name="email" type="email" inputMode="email" autoComplete={registering ? "email" : "username"} autoCapitalize="none" spellCheck={false} required value={email} disabled={busy} onChange={event => {
                setEmail(event.target.value)
                setNow(Date.now())
                setVerificationCode("")
                setSentEmail("")
                setErrors(current => ({ ...current, email: undefined, verification_code: undefined, form: undefined }))
              }} placeholder="name@example.com" className="h-11 items-center [&>input]:h-full [&>input]:leading-normal" aria-invalid={errors.email ? true : undefined} aria-describedby={errors.email ? "account-error" : undefined} />
            </AuthField>

            {registering && <AuthField id="verification-code" label="邮箱验证码" error={errors.verification_code} action={
              <Button type="button" variant="outline" size="sm" onClick={sendCode} disabled={busy || secondsRemaining > 0} className="h-8 rounded-full border-accent-border px-3 text-xs text-accent-text hover:border-accent-border hover:bg-accent-subtle">
                {sendingCode && <RotateCw className="size-3.5 motion-safe:animate-spin" aria-hidden="true" />}
                {sendingCode ? "发送中…" : secondsRemaining > 0 ? `${secondsRemaining} 秒后重发` : sentEmail ? "重新发送" : "获取验证码"}
              </Button>
            }>
              <OTPField id="verification-code" name="verification_code" aria-label="邮箱验证码" length={OTP_LENGTH} required disabled={busy} value={verificationCode} onValueChange={value => {
                setVerificationCode(value)
                clearError("verification_code")
              }} className="gap-1 sm:gap-2" aria-describedby={cn("verification-code-help", errors.verification_code && "verification-code-error")}>
                {OTP_SLOT_KEYS.map((slotKey, index) => (
                  <OTPFieldInput key={slotKey} ref={index === 0 ? codeRef : undefined} aria-label={index === 0 ? undefined : `验证码，第 ${index + 1} 位，共 ${OTP_LENGTH} 位`} aria-invalid={Boolean(errors.verification_code)} />
                ))}
              </OTPField>
              <p id="verification-code-help" className="text-xs leading-5 text-muted-foreground">输入邮件中的 6 位数字，5 分钟内有效。</p>
              <div role="status" aria-atomic="true" className={cn("flex items-start gap-2 text-xs leading-5 text-success-text", !sentEmail && "sr-only")}>
                {sentEmail && <><Check className="mt-0.5 size-3.5 shrink-0" strokeWidth={2} aria-hidden="true" /><span className="min-w-0 break-words">验证码已发送至 {sentEmail}</span></>}
              </div>
            </AuthField>}

            <AuthField id="password" label={registering ? "设置密码" : "密码"} hint={registering ? "至少 8 个字符" : undefined} error={errors.password}>
              <InputGroup className="h-11">
                <InputGroupInput ref={passwordRef} id="password" name="password" type={showPassword ? "text" : "password"} autoComplete={registering ? "new-password" : "current-password"} required minLength={registering ? 8 : undefined} value={password} disabled={busy} onChange={event => {
                  setPassword(event.target.value)
                  clearError("password")
                }} placeholder={registering ? "设置你的登录密码" : "输入密码"} className="[&>input]:h-10.5 [&>input]:leading-normal" aria-invalid={errors.password ? true : undefined} aria-describedby={cn(registering && "password-hint", errors.password && "password-error") || undefined} />
                <InputGroupAddon align="inline-end" className="py-0">
                  <Button type="button" variant="ghost" size="icon" className="size-9 rounded-full text-muted-foreground" disabled={busy} aria-label={showPassword ? "隐藏密码" : "显示密码"} aria-controls="password" aria-pressed={showPassword} onClick={() => setShowPassword(value => !value)}>
                    {showPassword ? <EyeOff aria-hidden="true" /> : <Eye aria-hidden="true" />}
                  </Button>
                </InputGroupAddon>
              </InputGroup>
            </AuthField>

            {registering && <AuthField id="display-name" label="显示名称" hint="可选">
              <Input id="display-name" name="name" autoComplete="nickname" value={displayName} disabled={busy} onChange={event => setDisplayName(event.target.value)} placeholder="怎么称呼你" className="h-11 items-center [&>input]:h-full [&>input]:leading-normal" />
            </AuthField>}

            {errors.form && <p role="alert" className="rounded-lg border border-danger-solid/20 bg-danger-subtle px-3.5 py-3 text-sm leading-6 text-danger-text">{errors.form}</p>}

            <Button type="submit" size="lg" className="h-11 w-full rounded-full shadow-xs" disabled={busy}>
              {loading && <RotateCw className="motion-safe:animate-spin" aria-hidden="true" />}
              <span>{loading ? registering ? "正在创建账户…" : "正在登录…" : registering ? "创建账户" : "登录"}</span>
              {!loading && <ArrowRight className="icon-directional" aria-hidden="true" />}
            </Button>
          </form>

          <p className="mt-6 text-center text-sm leading-6 text-muted-foreground">
            {registering ? "已有账户？" : "还没有账户？"}{" "}
            <Link href={registering ? "/login" : "/register"} className={authLinkClassName} onNavigate={event => { if (requestPending.current) event.preventDefault() }}>
              {registering ? "前往登录" : "创建账户"}
            </Link>
          </p>
          </div>
        </section>
      </div>
    </main>
  )
}

function AuthField({ id, label, hint, action, error, children }: { id: string; label: string; hint?: string; action?: ReactNode; error?: string; children: ReactNode }) {
  return (
    <Field className="gap-2" data-invalid={Boolean(error)}>
      <div className="flex min-h-5 items-center justify-between gap-3">
        <FieldLabel htmlFor={id} className="text-sm font-medium">{label}</FieldLabel>
        {hint && <FieldDescription id={`${id}-hint`} className="text-xs">{hint}</FieldDescription>}
        {action}
      </div>
      {children}
      {error && <FieldError id={`${id}-error`} className="leading-5 text-danger-text">{error}</FieldError>}
    </Field>
  )
}
