import Image from "next/image"
import { Server, Sparkles } from "lucide-react"
import type { ConsoleProvider } from "@/lib/api"

const icons: Record<string, string> = {
  openai: "openai.svg",
  anthropic: "claude.svg",
  gemini: "gemini.svg",
  antigravity: "antigravity.svg",
  grok: "grok.svg",
  kimi: "kimi.svg",
  zhipu: "zhipu.svg",
  deepseek: "deepseek.svg",
  custom: "custom.svg",
  "openai-compatible": "custom.svg",
}

type AvatarSource = Pick<ConsoleProvider, "provider_type" | "platform" | "avatar_url">

export function ProviderAvatar({ provider, size = 18 }: { provider: AvatarSource; size?: number }) {
  if (provider.avatar_url) {
    return <Image src={provider.avatar_url} alt="" width={size} height={size} unoptimized className="size-full rounded-[inherit] object-cover" />
  }

  const icon = icons[provider.platform || provider.provider_type]
  if (icon) {
    const url = `/provider-icons/${icon}`
    return <span aria-hidden="true" className="block shrink-0 bg-current" style={{ width: size, height: size, maskImage: `url(${url})`, WebkitMaskImage: `url(${url})`, maskPosition: "center", WebkitMaskPosition: "center", maskRepeat: "no-repeat", WebkitMaskRepeat: "no-repeat", maskSize: "contain", WebkitMaskSize: "contain" }} />
  }
  const Icon = provider.provider_type === "sub2api" && provider.platform !== "custom" ? Sparkles : Server
  return <Icon size={size} strokeWidth={1.75} aria-hidden="true" />
}
