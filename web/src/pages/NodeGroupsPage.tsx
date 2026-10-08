import { useState, type FormEvent } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import apiClient from '@/api/client'
import { usePagedList } from '@/api/page'
import { Button } from '@/components/Button'
import { DataTable, type Column } from '@/components/DataTable'

type NodeGroup = { id: string; name: string; description: string; domain_suffixes: string[]; wildcard_certificate_id: string | null; enabled: boolean }
type Certificate = { id: string; owner_user_id: string | null; domains: string[]; not_after: string }

export default function NodeGroupsPage() {
  const queryClient = useQueryClient()
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [suffixes, setSuffixes] = useState('')
  const [wildcardCertificateId, setWildcardCertificateId] = useState('')
  const [message, setMessage] = useState('')
  const groups = usePagedList<NodeGroup>(['admin-node-groups'], '/v1/admin/node-groups')
  const create = useMutation({
    mutationFn: () => apiClient.post('/v1/admin/node-groups', {
      name,
      description,
      domain_suffixes: suffixes.split(/[\s,，]+/).map((item) => item.trim()).filter(Boolean),
    }),
    onSuccess: async () => {
      setName('')
      setDescription('')
      setSuffixes('')
      setMessage('节点组已创建')
      await queryClient.invalidateQueries({ queryKey: ['admin-node-groups'] })
    },
    onError: (error) => setMessage(apiError(error)),
  })
  const columns: Column<NodeGroup>[] = [
    { key: 'name', title: '节点组' },
    { key: 'description', title: '描述' },
    { key: 'domain_suffixes', title: '域名后缀' },
    { key: 'enabled', title: '启用' },
  ]

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setMessage('')
    create.mutate()
  }

  return (
    <section className="page-container">
      <header className="page-header"><h1 className="page-title">节点组</h1></header>
      <form className="page-card mb-5 grid gap-4 p-5 md:grid-cols-2" onSubmit={submit}>
        <label><span className="apple-label">名称</span><input className="apple-input w-full" value={name} onChange={(event) => setName(event.target.value)} maxLength={100} required /></label>
        <label><span className="apple-label">描述</span><input className="apple-input w-full" value={description} onChange={(event) => setDescription(event.target.value)} maxLength={2048} /></label>
        <label className="md:col-span-2"><span className="apple-label">托管域名后缀（每行或逗号分隔）</span><textarea className="apple-input min-h-20 w-full" value={suffixes} onChange={(event) => setSuffixes(event.target.value)} /></label>
        {message && (
          <p className={`text-sm md:col-span-2 ${message.includes('登录') || message.includes('刷新') ? 'text-apple-red' : 'text-muted-foreground'}`} role="status">
            {message}
          </p>
        )}
        <div className="md:col-span-2"><Button type="submit" loading={create.isPending}>创建节点组</Button></div>
      </form>
      {groups.query.isError ? <p className="page-card p-5 text-sm text-apple-red" role="alert">{apiError(groups.query.error)}</p> : <DataTable columns={columns} data={groups.items} rowKey={(row) => row.id} loading={groups.query.isPending} pagination={groups.pagination} onPageChange={groups.setPage} onSizeChange={groups.setSize} />}
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
