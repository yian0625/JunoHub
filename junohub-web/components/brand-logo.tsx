import { useId } from "react"
import { cn } from "@/lib/utils"

export function BrandLogo({ className }: { className?: string }) {
  const id = `jcut-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`
  return (
    <span className={cn("grid shrink-0 place-items-center rounded-[28%] bg-white shadow-[0_1px_2px_rgba(16,24,40,.06),0_6px_16px_rgba(16,24,40,.10)] ring-1 ring-black/[.05]", className)}>
      <svg viewBox="0 0 1024 1024" className="size-[60%]" aria-hidden="true">
        <defs>
          <mask id={id} maskUnits="userSpaceOnUse" x="0" y="0" width="1024" height="1024">
            <rect width="1024" height="1024" fill="white" />
            <path d="M638 300 L806 226 L806 282 L638 356 Z" fill="black" />
          </mask>
        </defs>
        <g fill="none" stroke="#191715" strokeWidth="142" strokeLinecap="round" strokeLinejoin="round" mask={`url(#${id})`}>
          <path d="M344 190 H730 V610 C730 772 631 856 481 856 C348 856 258 786 238 670" />
        </g>
        <path d="M654 309 L790 249 L790 276 L654 336 Z" fill="#B4975A" />
      </svg>
    </span>
  )
}
