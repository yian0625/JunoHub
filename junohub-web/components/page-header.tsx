"use client"

import { ReactNode } from "react"

interface PageHeaderProps {
  title: string
  description?: string
  action?: ReactNode
}

export function PageHeader({ title, description, action }: PageHeaderProps) {
  return (
    <div className="page-header mb-6 flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
      <div>
        <h1 className="page-heading">
          <span>{title}</span>
        </h1>
        {description && (
          <p className="page-lead mt-2.5">{description}</p>
        )}
      </div>
      {action && <div>{action}</div>}
    </div>
  )
}
