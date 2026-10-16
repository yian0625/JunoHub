import * as React from "react"
import { Input as InputPrimitive } from "@base-ui/react/input"

import { cn } from "@/lib/utils"

function Input({ className, type, unstyled = false, ...props }: React.ComponentProps<"input"> & { unstyled?: boolean }) {
  return (
    <InputPrimitive
      type={type}
      data-slot="input"
      className={cn(
        "w-full min-w-0 text-base text-foreground outline-none file:inline-flex file:h-6 file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground placeholder:text-muted-foreground disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 sm:text-sm",
        !unstyled && "field-control h-11 px-3.5 py-1 aria-invalid:border-danger-solid aria-invalid:ring-3 aria-invalid:ring-danger-solid/15",
        className
      )}
      {...props}
    />
  )
}

export { Input }
