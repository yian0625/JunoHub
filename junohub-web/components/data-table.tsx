"use client"

import { cn } from "@/lib/utils"
import { ReactNode } from "react"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty"
import { Spinner } from "@/components/ui/spinner"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { ChevronLeft, ChevronRight } from "lucide-react"

export interface Column<T = object> {
  key: string
  title: string
  render?: (value: unknown, row: T, index: number) => ReactNode
  align?: "start" | "end"
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
  compact?: boolean
  hideSinglePage?: boolean
  tableClassName?: string
  emptyTitle?: string
  emptyDescription?: string
  emptyIcon?: ReactNode
  embedded?: boolean
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
  compact = false,
  hideSinglePage = false,
  tableClassName,
  emptyTitle = "暂无数据",
  emptyDescription,
  emptyIcon,
  embedded = false,
}: DataTableProps<T>) {
  const totalPages = Math.max(1, Math.ceil(total / pageSize))

  return (
    <Card className={cn("gap-0 py-0", embedded && "border-0 bg-transparent shadow-none")}>
          <Table className={cn("min-w-[760px] text-sm", tableClassName)} containerProps={{ role: "region", "aria-label": "数据表格，可横向滚动", tabIndex: 0, className: "overscroll-x-contain focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring" }}>
            <TableHeader>
              <TableRow className="bg-bg-sunken/60 hover:bg-bg-sunken/60">
                {columns.map((col) => (
                  <TableHead
                    key={col.key}
                    scope="col"
                    className={cn("whitespace-nowrap text-xs font-semibold text-muted-foreground", compact ? "px-4 py-2.5" : "px-5 py-3.5", col.align === "end" ? "text-end" : "text-start", col.className)}
                  >
                    {col.title}
                  </TableHead>
                ))}
              </TableRow>
            </TableHeader>
            <TableBody>
              {loading ? (
                <TableRow>
                  <TableCell
                    colSpan={columns.length}
                    className="h-32 text-center text-sm text-muted-foreground"
                  >
                    <div className="flex items-center justify-center gap-2" role="status">
                      <Spinner className="size-4" />
                      正在加载
                    </div>
                  </TableCell>
                </TableRow>
              ) : data.length === 0 ? (
                <TableRow>
                  <TableCell
                    colSpan={columns.length}
                    className="h-32 text-center text-sm text-muted-foreground"
                  >
                    <Empty className="border-0 py-6">
                      {emptyIcon && <EmptyMedia variant="icon">{emptyIcon}</EmptyMedia>}
                      <EmptyHeader><EmptyTitle>{emptyTitle}</EmptyTitle>{emptyDescription && <EmptyDescription>{emptyDescription}</EmptyDescription>}</EmptyHeader>
                    </Empty>
                  </TableCell>
                </TableRow>
              ) : (
                data.map((row, idx) => (
                  <TableRow
                    key={String(cellValue(row, rowKey) ?? idx)}
                    className="transition-colors duration-150 hover:bg-bg-sunken"
                  >
                    {columns.map((col) => (
                      <TableCell
                        key={col.key}
                        className={cn("whitespace-nowrap align-middle text-sm text-text-primary", compact ? "px-4 py-2.5" : "px-5 py-4", col.align === "end" && "text-end tabular-nums", col.className)}
                      >
                        {col.render
                          ? col.render(cellValue(row, col.key), row, idx)
                          : (cellValue(row, col.key) as ReactNode)}
                      </TableCell>
                    ))}
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>

      <p className="border-t border-border-default px-5 py-2 text-xs text-muted-foreground sm:hidden">左右滑动查看完整表格</p>

      {/* Pagination */}
      {total > 0 && (!hideSinglePage || totalPages > 1) && (
        <div className="resource-card-footer flex flex-col gap-3 border-t border-border-default px-5 py-4 sm:flex-row sm:items-center sm:justify-between">
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
    </Card>
  )
}
