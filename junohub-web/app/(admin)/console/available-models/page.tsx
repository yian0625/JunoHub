"use client"

import Link from "next/link"
import { useEffect, useState } from "react"
import { BookOpen, Network } from "lucide-react"
import { Button } from "@/components/coss-ui/button"
import { Card } from "@/components/ui/card"
import { consoleAPI, type AvailableModel } from "@/lib/api"
import { ModelCatalogCard, ModelCatalogDetails, ModelCatalogSearch, ModelCatalogGrid } from "@/components/model-catalog"
import { modelType, modelTypeLabels } from "@/lib/model-access"
import { modelInterface } from "@/lib/model-api"
import { ErrorState, LoadingState, NoSearchResults, PageHead } from "../_components/console-ui"
import { useConsoleGuard } from "../_components/use-console-guard"

const capabilityLabels = modelTypeLabels

export default function AvailableModelsPage() {
  const ready = useConsoleGuard()
  const [models, setModels] = useState<AvailableModel[]>([])
  const [query, setQuery] = useState("")
  const [type, setType] = useState("all")
  const [detail, setDetail] = useState<AvailableModel | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState("")
  const load = async () => {
    setLoading(true); setError("")
    try { setModels((await consoleAPI.availableModels()).list) }
    catch (error) { setError(error instanceof Error ? error.message : "无法加载可用模型") }
    finally { setLoading(false) }
  }
  useEffect(() => { if (ready) void load() }, [ready])
  if (loading) return <LoadingState />
  if (error) return <ErrorState message={error} onRetry={load} />
  const search = query.trim().toLowerCase()
  const visible = models.filter(model => (type === "all" || modelType(model.api_type) === type) && [model.alias, modelInterface(model.api_type).label, ...model.capabilities.map(value => capabilityLabels[value] || value)].join(" ").toLowerCase().includes(search))

  return <div className="view-enter space-y-8">
    <PageHead title="可用模型" description="选择模型名称，使用我的密钥发起调用。" action={<Button variant="outline" render={<Link href="/console/guide" />}><BookOpen aria-hidden="true" />查看接入指引</Button>} />
    <section className="space-y-4" aria-label="可用模型列表">
      <ModelCatalogSearch query={query} onQueryChange={setQuery} type={type} onTypeChange={setType} label="可用模型" count={visible.length} total={models.length} />
      {!models.length ? <Card className="p-8 text-center"><Network className="mx-auto size-6 text-muted-foreground" aria-hidden="true" /><h2 className="mt-4 text-base font-semibold">暂无可用模型</h2><p className="mt-2 text-sm text-muted-foreground">管理员开放模型后，会显示在这里。</p></Card> : !visible.length ? <NoSearchResults onClear={() => { setQuery(""); setType("all") }} /> : <ModelCatalogGrid items={visible} typeOf={model => modelType(model.api_type)}>{model => <ModelCatalogCard key={model.alias} model={model} onDetails={() => setDetail(model)} />}</ModelCatalogGrid>}
    </section>
    <ModelCatalogDetails model={detail} open={Boolean(detail)} onOpenChange={open => { if (!open) setDetail(null) }} />
  </div>
}
