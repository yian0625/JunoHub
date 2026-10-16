"use client"

import { useEffect, useState } from "react"
import { adminUserAPI, type AdminUserDetail } from "@/lib/api"
import { FormDialog } from "../console/_components/console-ui"
import { Button } from "@/components/ui/button"

const number = (value: number) => value.toLocaleString("zh-CN")
const date = (value: number) => value ? new Date(value * 1000).toLocaleString("zh-CN") : "暂无"
const money = (value: number) => `$${value.toLocaleString("en-US", { maximumFractionDigits: 6 })}`

export function UserDetail({ id, onClose }: { id: number; onClose: () => void }) {
  const [data, setData] = useState<AdminUserDetail | null>(null)
  const [error, setError] = useState("")
  const [now] = useState(() => Date.now() / 1000)
  const [retry, setRetry] = useState(0)
  useEffect(() => {
    let cancelled = false
    adminUserAPI.detail(id).then(result => { if (!cancelled) setData(result) })
      .catch(cause => { if (!cancelled) setError(cause instanceof Error ? cause.message : "无法加载用户详情") })
    return () => { cancelled = true }
  }, [id, retry])
  return <FormDialog key="user-detail" open onOpenChange={open => { if (!open) onClose() }} title="用户详情" description="账户信息、资源和密钥用量。" contentClassName="sm:max-w-2xl">
    {error ? <div role="alert" className="space-y-4"><p>{error}</p><Button variant="outline" onClick={() => { setError(""); setData(null); setRetry(value => value + 1) }}>重新加载</Button></div>
      : !data ? <p role="status" className="py-10 text-center text-sm text-muted-foreground">正在加载用户详情…</p>
      : <div className="space-y-6">
        <section aria-label="账户信息">
          <h2 className="break-words text-lg font-semibold">{data.user.display_name || data.user.email}</h2>
          <p className="mt-1 break-all text-sm text-muted-foreground">{data.user.email}</p>
          <dl className="mt-4 grid grid-cols-2 gap-x-6 gap-y-3 text-sm">
            {[["用户 ID", data.user.id], ["角色", data.user.role === "admin" ? "管理员" : "普通用户"], ["账户状态", data.user.status === 1 ? "正常" : "已停用"], ["最近使用密钥", date(data.usage.last_used_at)], ["注册时间", date(data.user.created_at)], ["更新时间", date(data.user.updated_at)]].map(([label, value]) => <div key={label} className="min-w-0"><dt className="text-xs text-muted-foreground">{label}</dt><dd className="mt-1 break-words tabular-nums">{value}</dd></div>)}
          </dl>
        </section>
        <section aria-label="资源与用量" className="border-t border-border-default pt-5">
          <h2 className="text-sm font-semibold">资源与用量</h2>
          <p className="mt-1 text-xs text-muted-foreground">用量为现有密钥的累计值，消耗按已配置价格估算。</p>
          <dl className="mt-4 grid grid-cols-2 gap-4 sm:grid-cols-3">
            {[["访问密钥", number(data.usage.keys_count)], ["供应商", number(data.usage.providers_count)], ["模型路由", number(data.usage.models_count)], ["请求数", number(data.usage.requests)], ["输入 Token", number(data.usage.input_tokens)], ["输出 Token", number(data.usage.output_tokens)], ["估算消耗（美元）", money(data.usage.cost_usd)]].map(([label, value]) => <div key={label}><dt className="text-xs text-muted-foreground">{label}</dt><dd className="mt-1 break-all text-lg font-semibold tabular-nums">{value}</dd></div>)}
          </dl>
          {data.usage.unpriced_requests > 0 && <p className="mt-3 text-xs text-muted-foreground">{number(data.usage.unpriced_requests)} 次请求未配置价格，未计入估算消耗。</p>}
        </section>
        <section aria-label="用户密钥" className="border-t border-border-default pt-5">
          <h2 className="text-sm font-semibold">访问密钥 <span className="font-normal text-muted-foreground">{number(data.usage.keys_count)}</span></h2>
          {!data.keys.length ? <p className="py-6 text-center text-sm text-muted-foreground">暂无访问密钥</p> : <ul className="mt-3 divide-y divide-border-default">{data.keys.map(key => <li key={key.id} className="flex items-start justify-between gap-4 py-3 text-sm"><div className="min-w-0"><p className="break-words font-medium">{key.name}</p><p className="mt-1 break-all font-mono text-xs text-muted-foreground">{key.prefix}••••••••</p><p className="mt-1 text-xs text-muted-foreground">{key.status !== 1 ? "已停用" : key.expires_at > 0 && key.expires_at <= now ? "已过期" : "已启用"}</p></div><div className="shrink-0 text-right tabular-nums"><p>{money(key.cost_usd)}</p><p className="mt-1 text-xs text-muted-foreground">{number(key.requests)} 次请求</p></div></li>)}</ul>}
          {data.usage.keys_count > 50 && <p className="mt-2 text-xs text-muted-foreground">显示最近创建的 50 个密钥。</p>}
        </section>
      </div>}
  </FormDialog>
}
