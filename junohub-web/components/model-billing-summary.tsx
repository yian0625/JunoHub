import type { BillingConfig } from "@/lib/model-billing"
import { billingUnitLabels } from "@/lib/model-billing"

const price = (value: number | null | undefined) => value == null ? "未配置" : `$${value > 0 && value < 1e-10 ? value.toExponential() : value.toLocaleString("en-US", { maximumFractionDigits: 10 })}`

export function ModelBillingSummary({ config, input, output, inputOnly }: {
  config?: Pick<BillingConfig, "unit" | "price_per_unit">; input?: number | null; output?: number | null; inputOnly?: boolean
}) {
  const unit = config?.unit || "token"
  return <div className="mt-3 flex flex-wrap items-baseline gap-x-4 gap-y-1 border-t border-border-default pt-2 text-xs tabular-nums">
    <span className="text-muted-foreground">$/{billingUnitLabels[unit]}</span>
    {unit === "token" ? <>
      <span><span className="text-muted-foreground">输入 </span><span className={input == null ? "text-muted-foreground" : "font-medium"}>{price(input)}</span></span>
      {!inputOnly && <span><span className="text-muted-foreground">输出 </span><span className={output == null ? "text-muted-foreground" : "font-medium"}>{price(output)}</span></span>}
    </> : <span className={config?.price_per_unit == null ? "text-muted-foreground" : "font-medium"}>{price(config?.price_per_unit)}</span>}
  </div>
}
