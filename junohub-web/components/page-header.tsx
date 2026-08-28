"use client"

import { ReactNode } from "react"

interface PageHeaderProps {
  title: string
  description?: string
  action?: ReactNode
}

export function PageHeader({ title, description, action }: PageHeaderProps) {
  return (
    <div className="page-header flex items-center justify-between mb-6">
      <div>
        <h1 className="page-title text-2xl font-semibold text-foreground tracking-tight flex items-center gap-2">
          <span className="page-title-dot accent-dot" />
          <span>{title}</span>
        </h1>
        {description && (
          <p className="page-description mt-1 text-sm text-muted-foreground">{description}</p>
        )}
      </div>
      {action && <div>{action}</div>}
    </div>
  )
}
