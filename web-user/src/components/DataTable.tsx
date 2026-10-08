import { useMemo, type ReactNode } from 'react'
import { Loader2 } from 'lucide-react'
import { formatCellDisplay } from '@/utils/formatDateTime'
import './DataTable.css'

export interface Column<T> {
  key: string
  title: string
  render?: (row: T) => ReactNode
}

interface DataTableProps<T> {
  columns: Column<T>[]
  data: T[]
  rowKey: (row: T) => string | number
  loading?: boolean
  emptyText?: string
  pagination?: { page: number; size: number; total: number }
  onPageChange?: (page: number) => void
  onSizeChange?: (size: number) => void
}

export function DataTable<T>({ columns, data, rowKey, loading = false, emptyText = '暂无数据', pagination, onPageChange, onSizeChange }: DataTableProps<T>) {
  const safeData = useMemo(() => Array.isArray(data) ? data : [], [data])
  const colSpan = Math.max(columns.length, 1)

  return (
    <div className="data-table-wrapper">
      <div className="data-table-body">
        <div className="data-table-scroll">
          <table className="data-table">
            <thead><tr>{columns.map((column) => <th key={column.key}>{column.title}</th>)}</tr></thead>
            <tbody>
              {loading ? <tr><td colSpan={colSpan}><div className="data-table__loading"><Loader2 size={20} className="data-table__spinner" /><span>加载中...</span></div></td></tr> : safeData.length === 0 ? <tr><td colSpan={colSpan}><div className="data-table__empty">{emptyText}</div></td></tr> : safeData.map((row) => (
                <tr key={rowKey(row)}>{columns.map((column) => <td key={column.key}>{column.render ? column.render(row) : displayValue(row, column.key)}</td>)}</tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
      {pagination ? (
        <ListPager
          page={pagination.page}
          size={pagination.size}
          total={pagination.total}
          onPageChange={onPageChange}
          onSizeChange={onSizeChange}
        />
      ) : null}
    </div>
  )
}

export function ListPager({
  page,
  size,
  total,
  onPageChange,
  onSizeChange,
}: {
  page: number
  size: number
  total: number
  onPageChange?: (page: number) => void
  onSizeChange?: (size: number) => void
}) {
  const totalPages = Math.max(1, Math.ceil(total / size))
  const from = total === 0 ? 0 : (page - 1) * size + 1
  const to = total === 0 ? 0 : Math.min(page * size, total)
  return (
    <nav className="mt-4 flex flex-wrap items-center justify-between gap-3" aria-label="分页">
      <span className="text-sm text-muted-foreground">第 {from}-{to} 条，共 {total} 条</span>
      <div className="flex flex-wrap items-center gap-2">
        <select className="apple-input" value={size} onChange={(event) => onSizeChange?.(Number(event.target.value))}>
          {[20, 50, 100].map((option) => <option key={option} value={option}>{option} 条/页</option>)}
        </select>
        <button type="button" className="apple-button-secondary" disabled={page <= 1} onClick={() => onPageChange?.(page - 1)}>上一页</button>
        <span className="text-sm text-muted-foreground">第 {Math.min(page, totalPages)} / {totalPages} 页</span>
        <button type="button" className="apple-button-secondary" disabled={page >= totalPages} onClick={() => onPageChange?.(page + 1)}>下一页</button>
      </div>
    </nav>
  )
}

function displayValue<T>(row: T, key: string): ReactNode {
  const value = (row as unknown as Record<string, unknown>)[key]
  return formatCellDisplay(key, value)
}
