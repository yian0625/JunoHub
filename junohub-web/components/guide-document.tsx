"use client"

import { useMemo } from "react"
import Markdown from "react-markdown"
import remarkGfm from "remark-gfm"
import remarkParse from "remark-parse"
import { unified } from "unified"
import type { Root, RootContent } from "mdast"
import { Copy, List } from "lucide-react"
import { Button } from "@/components/coss-ui/button"
import { copyModelText } from "@/components/model-catalog"

function textOf(node: RootContent | { value?: string; children?: unknown[] }): string {
  if ("value" in node && typeof node.value === "string") return node.value
  return "children" in node && node.children ? node.children.map(child => textOf(child as RootContent)).join("") : ""
}
function headingAnchors() {
  return (tree: Root) => {
    let index = 0
    for (const node of tree.children) if (node.type === "heading" && node.depth === 2) {
      node.data = { ...node.data, hProperties: { ...node.data?.hProperties, id: `guide-section-${index++}` } }
    }
  }
}
export function GuideDocument({ content, navigation = false }: {content: string; navigation?: boolean}) {
  const sections = useMemo(() => {
    const tree = unified().use(remarkParse).parse(content) as Root
    return tree.children.filter(node => node.type === "heading" && node.depth === 2).map((node, index) => ({id:`guide-section-${index}`,title:textOf(node)}))
  }, [content])
  const article = <div className="guide-markdown"><Markdown remarkPlugins={[remarkGfm, headingAnchors]} skipHtml components={{
    h1: ({children}) => <h2 className="guide-document-title">{children}</h2>,
    h2: ({children,id}) => <h3 id={id} className="scroll-mt-24">{children}</h3>,
    h3: ({children}) => <h4>{children}</h4>,
    pre: ({children}) => <pre tabIndex={0} aria-label="代码示例，可横向滚动">{children}</pre>,
  }}>{content}</Markdown></div>
  return <div className={navigation && sections.length ? "grid min-w-0 items-start gap-6 xl:grid-cols-[12rem_minmax(0,1fr)]" : "min-w-0"}>
    {navigation && sections.length > 0 && <nav aria-label="接入指引目录" className="min-w-0 space-y-3 xl:sticky xl:top-24"><h2 className="flex items-center gap-2 text-sm font-semibold"><List className="size-4" aria-hidden="true" />文档目录</h2><ul className="grid gap-1 sm:grid-cols-2 xl:grid-cols-1">{sections.map(section => <li key={section.id}><a className="block rounded-lg px-3 py-2 text-xs leading-5 text-muted-foreground transition-colors duration-150 hover:bg-bg-sunken hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring" href={`#${section.id}`}>{section.title}</a></li>)}</ul><Button variant="outline" size="sm" onClick={() => copyModelText(content)}><Copy aria-hidden="true" />复制全文</Button></nav>}
    <article className="min-w-0 rounded-2xl border border-border-default bg-bg-surface p-5 sm:p-8">{article}</article>
  </div>
}
