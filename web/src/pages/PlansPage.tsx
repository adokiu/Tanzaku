import { useState, type FormEvent, type ReactNode } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import apiClient from '@/api/client'
import { getPageItems, usePagedList } from '@/api/page'
import { Button } from '@/components/Button'
import { DataTable, type Column } from '@/components/DataTable'

type NodeGroup = { id: string; name: string; enabled: boolean }
type Plan = {
  id: string
  name: string
  description: string
  speed_limit_mbps: number
  max_conns_per_tunnel: number
  max_new_conns_per_sec: number
  max_tunnels: number
  allow_custom_port: boolean
  traffic_quota_bytes: number | null
  traffic_period: string
  duration_days: number | null
  allowed_protocols: string[]
  traffic_count_mode: string
  enabled: boolean
}
type PlanForm = {
  name: string
  description: string
  speed_limit_mbps: string
  max_conns_per_tunnel: string
  max_new_conns_per_sec: string
  max_tunnels: string
  allow_custom_port: boolean
  traffic_quota_bytes: string
  traffic_period: string
  duration_days: string
}

const initialForm: PlanForm = {
  name: '', description: '', speed_limit_mbps: '', max_conns_per_tunnel: '', max_new_conns_per_sec: '',
  max_tunnels: '', allow_custom_port: false, traffic_quota_bytes: '', traffic_period: 'month', duration_days: '',
}
const protocols = ['tcp', 'udp', 'http']
const periods = ['day', 'week', 'month', 'quarter', 'year', 'lifetime']

export default function PlansPage() {
  const queryClient = useQueryClient()
  const [form, setForm] = useState(initialForm)
  const [allowedProtocols, setAllowedProtocols] = useState<string[]>(['tcp'])
  const [groups, setGroups] = useState<string[]>([])
  const [message, setMessage] = useState('')
  const plans = usePagedList<Plan>(['admin-plans'], '/v1/admin/plans')
  const nodeGroups = useQuery({ queryKey: ['admin-node-groups'], queryFn: () => getPageItems<NodeGroup>('/v1/admin/node-groups') })
  const create = useMutation({
    mutationFn: () => apiClient.post('/v1/admin/plans', {
      ...form,
      speed_limit_mbps: Number(form.speed_limit_mbps),
      max_conns_per_tunnel: Number(form.max_conns_per_tunnel),
      max_new_conns_per_sec: Number(form.max_new_conns_per_sec),
      max_tunnels: Number(form.max_tunnels),
      traffic_quota_bytes: form.traffic_quota_bytes ? Number(form.traffic_quota_bytes) : null,
      duration_days: form.duration_days ? Number(form.duration_days) : null,
      allowed_protocols: allowedProtocols,
      node_group_ids: groups,
      traffic_count_mode: 'sum',
    }),
    onSuccess: async () => {
      setForm(initialForm)
      setAllowedProtocols(['tcp'])
      setGroups([])
      setMessage('套餐已创建')
      await queryClient.invalidateQueries({ queryKey: ['admin-plans'] })
    },
    onError: (error) => setMessage(apiError(error)),
  })
  const columns: Column<Plan>[] = [
    { key: 'name', title: '套餐' },
    { key: 'speed_limit_mbps', title: 'Mbps' },
    { key: 'max_conns_per_tunnel', title: '单隧道并发' },
    { key: 'max_new_conns_per_sec', title: '新建连接/秒' },
    { key: 'max_tunnels', title: '最大隧道数' },
    { key: 'allow_custom_port', title: '自定义端口' },
    { key: 'traffic_period', title: '流量周期' },
    { key: 'enabled', title: '启用' },
  ]

  function update<K extends keyof PlanForm>(key: K, value: PlanForm[K]) {
    setForm((current) => ({ ...current, [key]: value }))
    setMessage('')
  }

  function toggle(setter: (value: string[]) => void, current: string[], value: string) {
    setter(current.includes(value) ? current.filter((item) => item !== value) : [...current, value])
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setMessage('')
    create.mutate()
  }

  return <section className="page-container">
    <header className="page-header"><h1 className="page-title">套餐管理</h1></header>
    <form className="page-card mb-5 grid gap-4 p-5 md:grid-cols-2" onSubmit={submit}>
      <Field label="套餐名称"><input className="apple-input w-full" value={form.name} onChange={(event) => update('name', event.target.value)} maxLength={100} required /></Field>
      <Field label="描述"><input className="apple-input w-full" value={form.description} onChange={(event) => update('description', event.target.value)} /></Field>
      <Field label="每隧道限速 Mbps"><input className="apple-input w-full" type="number" min="1" value={form.speed_limit_mbps} onChange={(event) => update('speed_limit_mbps', event.target.value)} required /></Field>
      <Field label="单隧道最大并发"><input className="apple-input w-full" type="number" min="1" value={form.max_conns_per_tunnel} onChange={(event) => update('max_conns_per_tunnel', event.target.value)} required /></Field>
      <Field label="单隧道每秒新建上限"><input className="apple-input w-full" type="number" min="1" value={form.max_new_conns_per_sec} onChange={(event) => update('max_new_conns_per_sec', event.target.value)} required /></Field>
      <Field label="用户最大隧道数"><input className="apple-input w-full" type="number" min="1" value={form.max_tunnels} onChange={(event) => update('max_tunnels', event.target.value)} required /></Field>
      <Field label="流量周期"><select className="apple-input w-full" value={form.traffic_period} onChange={(event) => update('traffic_period', event.target.value)}>{periods.map((period) => <option key={period} value={period}>{period}</option>)}</select></Field>
      <Field label="订阅天数（长期留空）"><input className="apple-input w-full" type="number" min="1" value={form.duration_days} onChange={(event) => update('duration_days', event.target.value)} /></Field>
      <Field label="周期流量配额（字节，留空不限）"><input className="apple-input w-full" type="number" min="1" value={form.traffic_quota_bytes} onChange={(event) => update('traffic_quota_bytes', event.target.value)} /></Field>
      <fieldset><legend className="apple-label">允许的上层协议</legend><div className="flex flex-wrap gap-4">{protocols.map((protocol) => <label className="flex items-center gap-2 text-sm" key={protocol}><input type="checkbox" checked={allowedProtocols.includes(protocol)} onChange={() => toggle(setAllowedProtocols, allowedProtocols, protocol)} />{protocol.toUpperCase()}</label>)}</div></fieldset>
      <fieldset><legend className="apple-label">允许的节点组</legend><div className="flex flex-wrap gap-4">{(nodeGroups.data ?? []).filter((group) => group.enabled).map((group) => <label className="flex items-center gap-2 text-sm" key={group.id}><input type="checkbox" checked={groups.includes(group.id)} onChange={() => toggle(setGroups, groups, group.id)} />{group.name}</label>)}</div></fieldset>
      <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={form.allow_custom_port} onChange={(event) => update('allow_custom_port', event.target.checked)} />允许用户自定义公网端口</label>
      {message && <p className="text-sm text-muted-foreground md:col-span-2" role="status">{message}</p>}
      <div className="md:col-span-2"><Button type="submit" loading={create.isPending} disabled={nodeGroups.isPending}>创建套餐</Button></div>
    </form>
    {plans.query.isError ? <p className="page-card p-5 text-sm text-apple-red" role="alert">{apiError(plans.query.error)}</p> : <DataTable columns={columns} data={plans.items} rowKey={(row) => row.id} loading={plans.query.isPending} pagination={plans.pagination} onPageChange={plans.setPage} onSizeChange={plans.setSize} />}
  </section>
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
