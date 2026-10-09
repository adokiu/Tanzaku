import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { usePagedList } from '@/api/page'
import { Link, useNavigate } from 'react-router-dom'
import apiClient from '@/api/client'
import { Button } from '@/components/Button'
import { DataTable, type Column } from '@/components/DataTable'

type Tunnel = {
  id: string
  name: string
  node_id: string
  node_name?: string | null
  client_id: string
  client_name?: string | null
  carrier: string
  protocol: string
  remote_port?: number | null
  target_host?: string | null
  target_port?: number | null
  target_url?: string | null
  status: string
  speed_limit_mbps: number
  max_conns: number
  max_new_conns_per_sec: number
  last_error?: string | null
}

export default function TunnelListPage() {
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [message, setMessage] = useState('')
  const tunnels = usePagedList<Tunnel>(['user-tunnels'], '/v1/tunnels')
  const remove = useMutation({
    mutationFn: (id: string) => apiClient.delete(`/v1/tunnels/${id}`),
    onSuccess: async () => {
      setMessage('隧道已删除，释放端口正在同步')
      await queryClient.invalidateQueries({ queryKey: ['user-tunnels'] })
    },
    onError: (error) => setMessage(apiError(error)),
  })
  const columns: Column<Tunnel>[] = [
    {
      key: 'name',
      title: '隧道名称',
      render: (row) => <Link className="text-primary hover:underline" to={`/tunnels/${row.id}`}>{row.name}</Link>,
    },
    { key: 'node_name', title: 'Server 节点' },
    { key: 'client_name', title: 'Client' },
    { key: 'carrier', title: 'Carrier' },
    { key: 'protocol', title: '协议' },
    { key: 'remote_port', title: '公网端口' },
    { key: 'status', title: '状态' },
    { key: 'speed_limit_mbps', title: '限速 Mbps' },
    { key: 'max_conns', title: '并发上限' },
    { key: 'max_new_conns_per_sec', title: '新建连接/秒' },
  ]

  return (
    <section className="page-container">
      <header className="page-header flex items-center justify-between gap-4"><h1 className="page-title">我的隧道</h1><Button type="button" onClick={() => navigate('/tunnels/new')}>创建隧道</Button></header>
      {message && <p className="mb-4 text-sm text-muted-foreground" role="status">{message}</p>}
      {tunnels.query.isError ? <p className="page-card p-5 text-sm text-apple-red" role="alert">{apiError(tunnels.query.error)}</p> : <DataTable columns={columns} data={tunnels.items} rowKey={(row) => row.id} loading={tunnels.query.isPending} pagination={tunnels.pagination} onPageChange={tunnels.setPage} onSizeChange={tunnels.setSize} />}
      {tunnels.items.length > 0 && <div className="mt-4 flex flex-wrap gap-2">{tunnels.items.map((tunnel) => (
        <div className="page-card flex w-full items-center justify-between gap-4 p-4" key={tunnel.id}>
          <div className="min-w-0">
            <p className="truncate font-medium">{tunnel.name}</p>
            <div className="mt-1 flex min-w-0 flex-wrap items-center gap-2 text-sm text-muted-foreground">
              <span className="truncate">{tunnel.target_url ?? `${tunnel.target_host}:${tunnel.target_port}`}</span>
              <span className="rounded border border-border bg-muted px-2 py-0.5 text-xs text-muted-foreground">{tunnel.last_error ?? tunnel.status}</span>
            </div>
          </div>
          <Button type="button" variant="secondary" loading={remove.isPending} onClick={() => remove.mutate(tunnel.id)}>删除</Button>
        </div>
      ))}</div>}
    </section>
  )
}

function apiError(error: unknown): string {
  if (typeof error === 'object' && error !== null && 'response' in error) {
    const response = (error as { response?: { data?: { error?: string } } }).response
    if (response?.data?.error) return response.data.error
  }
  return '无法连接 board'
}
