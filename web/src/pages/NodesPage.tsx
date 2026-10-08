import { useState, type FormEvent, type ReactNode } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import apiClient from '@/api/client'
import { getPageItems, usePagedList } from '@/api/page'
import { Button } from '@/components/Button'
import { DataTable, type Column } from '@/components/DataTable'

type NodeGroup = { id: string; name: string; enabled: boolean }
type Node = {
  id: string
  name: string
  region: string
  public_host: string
  bind_addr: string
  enabled: boolean
  online: boolean
  carrier_ports: Record<string, { port: number; enabled: boolean }>
  tcp_port_ranges: number[][]
  udp_port_ranges: number[][]
  http_shared_port: number
  https_shared_port: number
  last_seen_at: string | null
}
type NodeForm = {
  name: string
  region: string
  public_host: string
  bind_addr: string
  node_group_id: string
  tcp_carrier_port: string
  quic_carrier_port: string
  tcp_port_ranges: string
  udp_port_ranges: string
  port_exclude: string
  http_shared_port: string
  https_shared_port: string
}

const initialForm: NodeForm = {
  name: '', region: '', public_host: '', bind_addr: '0.0.0.0', node_group_id: '',
  tcp_carrier_port: '7000', quic_carrier_port: '7000',
  tcp_port_ranges: '20000-29999', udp_port_ranges: '20000-29999', port_exclude: '',
  http_shared_port: '80', https_shared_port: '443',
}

export default function NodesPage() {
  const queryClient = useQueryClient()
  const [form, setForm] = useState(initialForm)
  const [message, setMessage] = useState('')
  const [issuedToken, setIssuedToken] = useState('')
  const groups = useQuery({
    queryKey: ['admin-node-groups'],
    queryFn: () => getPageItems<NodeGroup>('/v1/admin/node-groups'),
  })
  const nodes = usePagedList<Node>(['admin-nodes'], '/v1/admin/nodes')
  const create = useMutation({
    mutationFn: async () => (await apiClient.post('/v1/admin/nodes', {
      ...form,
      tcp_carrier_port: Number(form.tcp_carrier_port),
      quic_carrier_port: Number(form.quic_carrier_port),
      http_shared_port: Number(form.http_shared_port),
      https_shared_port: Number(form.https_shared_port),
      bind_addr: form.bind_addr || null,
      node_group_id: form.node_group_id,
    })).data as { id: string; token: string; token_prefix: string },
    onSuccess: async (node) => {
      setIssuedToken(node.token)
      setMessage('节点已创建。完整 Token 仅显示本次，请立即保存。')
      setForm(initialForm)
      await queryClient.invalidateQueries({ queryKey: ['admin-nodes'] })
      await queryClient.invalidateQueries({ queryKey: ['admin-node-groups'] })
    },
    onError: (error) => setMessage(apiError(error)),
  })
  const columns: Column<Node>[] = [
    { key: 'name', title: '节点' },
    { key: 'region', title: '地区' },
    { key: 'public_host', title: '公网地址' },
    { key: 'online', title: '在线' },
    { key: 'carrier_ports', title: 'Carrier 端口' },
    { key: 'tcp_port_ranges', title: 'TCP 端口范围' },
    { key: 'udp_port_ranges', title: 'UDP 端口范围' },
    { key: 'last_seen_at', title: '最近心跳' },
  ]

  function update<K extends keyof NodeForm>(key: K, value: NodeForm[K]) {
    setForm((current) => ({ ...current, [key]: value }))
    setMessage('')
    setIssuedToken('')
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setMessage('')
    setIssuedToken('')
    create.mutate()
  }

  return (
    <section className="page-container">
      <header className="page-header"><h1 className="page-title">Server 节点</h1></header>
      <form className="page-card mb-5 grid gap-4 p-5 md:grid-cols-2" onSubmit={submit}>
        <Field label="节点名称"><input className="apple-input w-full" value={form.name} onChange={(event) => update('name', event.target.value)} maxLength={100} required /></Field>
        <Field label="地区"><input className="apple-input w-full" value={form.region} onChange={(event) => update('region', event.target.value)} maxLength={100} /></Field>
        <Field label="公网地址"><input className="apple-input w-full" value={form.public_host} onChange={(event) => update('public_host', event.target.value)} maxLength={253} required /></Field>
        <Field label="监听绑定地址"><input className="apple-input w-full" value={form.bind_addr} onChange={(event) => update('bind_addr', event.target.value)} /></Field>
        <Field label="节点组"><select className="apple-input w-full" value={form.node_group_id} onChange={(event) => update('node_group_id', event.target.value)} required><option value="">选择节点组</option>{groups.data?.filter((group) => group.enabled).map((group) => <option key={group.id} value={group.id}>{group.name}</option>)}</select></Field>
        <div className="grid grid-cols-2 gap-3">
          <Field label="TCP Carrier 端口"><input className="apple-input w-full" type="number" min="1" max="65535" value={form.tcp_carrier_port} onChange={(event) => update('tcp_carrier_port', event.target.value)} required /></Field>
          <Field label="QUIC Carrier 端口"><input className="apple-input w-full" type="number" min="1" max="65535" value={form.quic_carrier_port} onChange={(event) => update('quic_carrier_port', event.target.value)} required /></Field>
        </div>
        <Field label="TCP 公网端口范围"><input className="apple-input w-full" value={form.tcp_port_ranges} onChange={(event) => update('tcp_port_ranges', event.target.value)} required /></Field>
        <Field label="UDP 公网端口范围"><input className="apple-input w-full" value={form.udp_port_ranges} onChange={(event) => update('udp_port_ranges', event.target.value)} required /></Field>
        <Field label="排除端口范围"><input className="apple-input w-full" value={form.port_exclude} onChange={(event) => update('port_exclude', event.target.value)} /></Field>
        <div className="grid grid-cols-2 gap-3">
          <Field label="共享 HTTP 端口"><input className="apple-input w-full" type="number" min="1" max="65535" value={form.http_shared_port} onChange={(event) => update('http_shared_port', event.target.value)} required /></Field>
          <Field label="共享 HTTPS 端口"><input className="apple-input w-full" type="number" min="1" max="65535" value={form.https_shared_port} onChange={(event) => update('https_shared_port', event.target.value)} required /></Field>
        </div>
        {message && <p className="text-sm text-muted-foreground md:col-span-2" role="status">{message}</p>}
        {issuedToken && <Field label="一次性节点 Token"><input className="apple-input w-full font-mono" readOnly value={issuedToken} /></Field>}
        <div className="md:col-span-2"><Button type="submit" loading={create.isPending} disabled={groups.isPending || !groups.data?.some((group) => group.enabled)}>创建节点</Button></div>
      </form>
      {nodes.query.isError ? <p className="page-card p-5 text-sm text-apple-red" role="alert">{apiError(nodes.query.error)}</p> : <DataTable columns={columns} data={nodes.items} rowKey={(row) => row.id} loading={nodes.query.isPending} pagination={nodes.pagination} onPageChange={nodes.setPage} onSizeChange={nodes.setSize} />}
    </section>
  )
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return <label className="block"><span className="apple-label">{label}</span>{children}</label>
}

function apiError(error: unknown): string {
  if (typeof error === 'object' && error !== null && 'response' in error) {
    const response = (error as { response?: { data?: { error?: string } } }).response
    if (response?.data?.error) return response.data.error
  }
  return '无法连接 board'
}
