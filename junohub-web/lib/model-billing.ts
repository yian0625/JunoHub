export type BillingUnit = "token" | "image" | "second" | "character" | "request"
export interface BillingConfig { unit: BillingUnit; price_per_unit: number | null; quantity_pointer: string }
export const billingUnits = [
  { value: "token", label: "按 Token" }, { value: "image", label: "按张" },
  { value: "second", label: "按秒" }, { value: "character", label: "按字符" },
  { value: "request", label: "按次" },
]
export const billingUnitLabels: Record<BillingUnit, string> = { token: "百万 Token", image: "张", second: "秒", character: "字符", request: "次" }
export const requiresQuantityField = (unit: BillingUnit) => unit === "second" || unit === "character"
