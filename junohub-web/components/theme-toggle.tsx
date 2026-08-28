"use client"

import { useSyncExternalStore } from "react"
import { Check, Monitor, Moon, Sun, type LucideIcon } from "lucide-react"
import { useTheme } from "next-themes"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { cn } from "@/lib/utils"

type ThemeOption = {
  value: "light" | "dark" | "system"
  label: string
  icon: LucideIcon
}

const themeOptions: ThemeOption[] = [
  { value: "light", label: "浅色", icon: Sun },
  { value: "dark", label: "深色", icon: Moon },
  { value: "system", label: "跟随系统", icon: Monitor },
]

export function ThemeToggle() {
  const { theme, resolvedTheme, setTheme } = useTheme()
  const mounted = useSyncExternalStore(() => () => undefined, () => true, () => false)
  const currentTheme = mounted ? theme || "system" : "system"
  const TriggerIcon = currentTheme === "system" ? Monitor : resolvedTheme === "dark" ? Moon : Sun

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        className="inline-flex size-10 items-center justify-center rounded-lg text-muted-foreground transition-[color,background-color] duration-150 ease-out hover:bg-bg-sunken hover:text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:scale-[0.96]"
        aria-label="切换主题"
      >
        <TriggerIcon className="h-4 w-4" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" sideOffset={8} className="w-44 rounded-xl p-1.5">
        {themeOptions.map((option) => {
          const Icon = option.icon
          const selected = currentTheme === option.value
          return (
            <DropdownMenuItem
              key={option.value}
              onClick={() => setTheme(option.value)}
              className={cn("gap-3 rounded-lg px-3 py-2 text-sm", selected && "bg-accent text-accent-foreground")}
            >
              <Icon className="h-4 w-4 text-muted-foreground" />
              <span className="flex-1">{option.label}</span>
              {selected && <Check className="h-4 w-4 text-primary" />}
            </DropdownMenuItem>
          )
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
