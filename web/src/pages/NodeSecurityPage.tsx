import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import apiClient from '@/api/client'
import { getPageItems } from '@/api/page'
import { Button } from '@/components/Button'

type NodeSecurity = { id: string; name: string; guard_policy: Record<string, unknown>; trusted_proxies: string[] }

export default function NodeSecurityPage() {
  const queryClient = useQueryClient()
  const [nodeId, setNodeId] = useState('')
  const [guardPolicy, setGuardPolicy] = useState('')
  const [trustedProxies, setTrustedProxies] = useState('')
  const [message, setMessage] = useState('')
  const query = useQuery({
    queryKey: ['admin-security'],
    queryFn: () => getPageItems<NodeSecurity>('/v1/admin/security'),
  })
  const selected = query.data?.find((node) => node.id === nodeId)
  const update = useMutation({
    mutationFn: () => apiClient.put(`/v1/admin/security/${nodeId}`, {
      guard_policy: JSON.parse(guardPolicy),
      trusted_proxies: trustedProxies.split(/[\s,，]+/).map((value) => value.trim()).filter(Boolean),
    }),
    onSuccess: async () => {
      setMessage('节点防护配置已保存')
      await queryClient.invalidateQueries({ queryKey: ['admin-security'] })
    },
    onError: (error) => setMessage(apiError(error)),
  })

  function selectNode(id: string) {
    setNodeId(id)
    const node = query.data?.find((item) => item.id === id)
    setGuardPolicy(node ? JSON.stringify(node.guard_policy, null, 2) : '')
    setTrustedProxies(node?.trusted_proxies.join('\n') ?? '')
    setMessage('')
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setMessage('')
    try {
      const policy = JSON.parse(guardPolicy) as unknown
      if (!policy || typeof policy !== 'object' || Array.isArray(policy)) {
        setMessage('防护策略 JSON 须为对象')
        return
      }
      update.mutate()
    } catch {
      setMessage('防护策略 JSON 格式无效')
    }
  }

  return <section className="page-container">
    <header className="page-header"><h1 className="page-title">节点防护策略</h1></header>
    {query.isError ? <p className="page-card p-5 text-sm text-apple-red" role="alert">{apiError(query.error)}</p> : (
      <form className="page-card grid gap-5 p-5" onSubmit={submit}>
        <label><span className="apple-label">节点</span><select className="apple-input w-full" value={nodeId} onChange={(event) => selectNode(event.target.value)} required><option value="">选择节点</option>{query.data?.map((node) => <option key={node.id} value={node.id}>{node.name}</option>)}</select></label>
        {selected && <>
          <label><span className="apple-label">Guard 模块 JSON 配置</span><textarea className="apple-input min-h-64 w-full font-mono text-xs" spellCheck={false} value={guardPolicy} onChange={(event) => setGuardPolicy(event.target.value)} /></label>
          <label><span className="apple-label">可信上游 CIDR（每行一条）</span><textarea className="apple-input min-h-24 w-full font-mono text-xs" value={trustedProxies} onChange={(event) => setTrustedProxies(event.target.value)} /></label>
          <p className="text-sm text-muted-foreground">配置适用于所选节点；未知模块或无效 CIDR 会被后端拒绝。</p>
        </>}
        {message && <p className="text-sm text-muted-foreground" role="status">{message}</p>}
        <div><Button type="submit" loading={update.isPending} disabled={!selected}>保存防护配置</Button></div>
      </form>
    )}
  </section>
}

function apiError(error: unknown): string {
  if (typeof error === 'object' && error !== null && 'response' in error) {
    const response = (error as { response?: { data?: { error?: string } } }).response
    if (response?.data?.error) return response.data.error
  }
  return '无法连接 board'
}
