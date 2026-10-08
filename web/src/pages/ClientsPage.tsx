import { useState, type FormEvent } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import apiClient from '@/api/client'
import { usePagedList } from '@/api/page'
import { Button } from '@/components/Button'
import { DataTable, type Column } from '@/components/DataTable'

type Client = {
  id: string
  user_id: string
  user_email?: string | null
  name: string
  enabled: boolean
  online: boolean
  capabilities: { carriers?: string[] }
  certificate_fingerprint?: string | null
  version?: string | null
  os?: string | null
  last_seen_at?: string | null
}

export default function ClientsPage({ audience }: { audience: 'admin' | 'user' }) {
  const queryClient = useQueryClient()
  const [name, setName] = useState('')
  const [token, setToken] = useState('')
  const [message, setMessage] = useState('')
  const endpoint = audience === 'admin' ? '/v1/admin/clients' : '/v1/clients'
  const clients = usePagedList<Client>(['clients', audience], endpoint)
  const create = useMutation({
    mutationFn: async () => (await apiClient.post('/v1/clients', { name })).data as { token: string },
    onSuccess: async (result) => {
      setToken(result.token)
      setMessage('Client 已创建；完整 Token 仅显示本次，请立即保存。')
      setName('')
      await queryClient.invalidateQueries({ queryKey: ['clients'] })
    },
    onError: (error) => setMessage(apiError(error)),
  })
  const columns: Column<Client>[] = [
    ...(audience === 'admin' ? [{ key: 'user_email' as const, title: '用户' }] : []),
    { key: 'name', title: 'Client' },
    { key: 'online', title: '在线' },
    { key: 'capabilities', title: '能力' },
    { key: 'certificate_fingerprint', title: '证书指纹' },
    { key: 'version', title: '版本' },
    { key: 'last_seen_at', title: '最近心跳' },
  ]

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setMessage('')
    setToken('')
    create.mutate()
  }

  return <section className="page-container">
    <header className="page-header"><h1 className="page-title">{audience === 'admin' ? 'Client 管理' : '我的 Client'}</h1></header>
    {audience === 'user' && <form className="page-card mb-5 flex flex-wrap items-end gap-4 p-5" onSubmit={submit}>
      <label className="min-w-64 flex-1"><span className="apple-label">Client 名称</span><input className="apple-input w-full" value={name} onChange={(event) => setName(event.target.value)} maxLength={100} required /></label>
      <Button type="submit" loading={create.isPending}>创建 Client</Button>
      {message && <p className="w-full text-sm text-muted-foreground" role="status">{message}</p>}
      {token && <div className="w-full"><span className="apple-label">一次性 Token</span><pre className="overflow-x-auto rounded-lg bg-muted p-3 text-xs">{`tanzaku-client --board <agent-websocket-address> --token ${token}`}</pre></div>}
    </form>}
    {audience === 'admin' && message && <p className="mb-4 text-sm text-muted-foreground" role="status">{message}</p>}
    {clients.query.isError ? <p className="page-card p-5 text-sm text-apple-red" role="alert">{apiError(clients.query.error)}</p> : <DataTable columns={columns} data={clients.items} rowKey={(row) => row.id} loading={clients.query.isPending} pagination={clients.pagination} onPageChange={clients.setPage} onSizeChange={clients.setSize} />}
  </section>
}

function apiError(error: unknown): string {
  if (typeof error === 'object' && error !== null && 'response' in error) {
    const response = (error as { response?: { data?: { error?: string } } }).response
    if (response?.data?.error) return response.data.error
  }
  return '无法连接 board'
}
