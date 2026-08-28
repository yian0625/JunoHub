"use client"

import { ReactNode } from "react"
import { Button } from "@/components/ui/button"
import { ChevronLeft, ChevronRight } from "lucide-react"

export interface Column<T = object> {
  key: string
  title: string
  render?: (value: unknown, row: T, index: number) => ReactNode
  className?: string
}

interface DataTableProps<T = object> {
  columns: Column<T>[]
  data: T[]
  page: number
  pageSize: number
  total: number
  onPageChange: (page: number) => void
  loading?: boolean
  rowKey?: string
}

function cellValue<T extends object>(row: T, key: string): unknown {
  return (row as Record<string, unknown>)[key]
}

export function DataTable<T extends object = Record<string, unknown>>({
  columns,
  data,
  page,
  pageSize,
  total,
  onPageChange,
  loading = false,
  rowKey = "id",
}: DataTableProps<T>) {
  const totalPages = Math.max(1, Math.ceil(total / pageSize))

  return (
    <div className="space-y-4">
      <div className="overflow-hidden rounded-2xl border border-border-subtle bg-bg-surface shadow-[var(--shadow-surface)]">
        <div className="overflow-x-auto overscroll-x-contain">
          <table className="min-w-[760px] w-full text-sm">
            <thead>
              <tr className="border-b border-border-subtle bg-bg-sunken">
                {columns.map((col) => (
                  <th
                    key={col.key}
                    className={`whitespace-nowrap px-5 py-3.5 text-left text-xs font-semibold text-muted-foreground ${col.className || ""}`}
                  >
                    {col.title}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody className="divide-y divide-border-subtle">
              {loading ? (
                <tr>
                  <td
                    colSpan={columns.length}
                    className="h-32 text-center text-sm text-muted-foreground"
                  >
                    <div className="flex items-center justify-center gap-2" role="status">
                      <div className="size-4 animate-spin rounded-full border-2 border-muted-foreground/20 border-t-muted-foreground/60" aria-hidden="true" />
                      正在加载
                    </div>
                  </td>
                </tr>
              ) : data.length === 0 ? (
                <tr>
                  <td
                    colSpan={columns.length}
                    className="h-32 text-center text-sm text-muted-foreground"
                  >
                    暂无数据
                  </td>
                </tr>
              ) : (
                data.map((row, idx) => (
                  <tr
                    key={String(cellValue(row, rowKey) ?? idx)}
                    className="transition-colors duration-150 hover:bg-bg-raised"
                  >
                    {columns.map((col) => (
                      <td
                        key={col.key}
                        className={`whitespace-nowrap px-5 py-4 align-middle text-sm text-text-primary ${col.className || ""}`}
                      >
                        {col.render
                          ? col.render(cellValue(row, col.key), row, idx)
                          : (cellValue(row, col.key) as ReactNode)}
                      </td>
                    ))}
                  </tr>
                ))
              )}
            </tbody>
          </table>
        </div>
      </div>

      {/* Pagination */}
      {total > 0 && (
        <div className="flex flex-col gap-3 px-1 sm:flex-row sm:items-center sm:justify-between">
          <p className="text-xs font-medium text-muted-foreground">
            共 {total} 条，第 {page} / {totalPages} 页
          </p>
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() => onPageChange(page - 1)}
              disabled={page <= 1}
              className="px-3 text-xs"
            >
              <ChevronLeft className="mr-1 size-4" aria-hidden="true" />
              上一页
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => onPageChange(page + 1)}
              disabled={page >= totalPages}
              className="px-3 text-xs"
            >
              下一页
              <ChevronRight className="ml-1 size-4" aria-hidden="true" />
            </Button>
          </div>
        </div>
      )}
    </div>
  )
}
