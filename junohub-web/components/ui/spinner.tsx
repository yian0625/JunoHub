import { LoaderCircle } from "lucide-react"
import { cn } from "@/lib/utils"

export function Spinner({ className, ...props }: React.ComponentProps<typeof LoaderCircle>) {
  return <LoaderCircle data-slot="spinner" role="status" aria-label="正在加载" className={cn("size-4 animate-spin", className)} {...props} />
}
