// Adapted from shadcn/ui Base UI source, commit 0132174664c07d41262fb51012d0cc782e458e6c.
"use client"

import { Switch as SwitchPrimitive } from "@base-ui/react/switch"
import { cn } from "@/lib/utils"

function Switch({ className, size = "default", ...props }: SwitchPrimitive.Root.Props & { size?: "sm" | "default" }) {
  return <SwitchPrimitive.Root data-slot="switch" data-size={size} className={cn("cn-switch peer group/switch relative inline-flex items-center transition-all outline-none after:absolute after:-inset-x-3 after:-inset-y-3.5 data-disabled:cursor-not-allowed data-disabled:opacity-50", className)} {...props}>
    <SwitchPrimitive.Thumb data-slot="switch-thumb" className="cn-switch-thumb pointer-events-none block ring-0 transition-transform" />
  </SwitchPrimitive.Root>
}

export { Switch }
