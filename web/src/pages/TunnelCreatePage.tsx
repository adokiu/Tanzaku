import { useEffect, useMemo, useState, type FormEvent, type ReactNode } from 'react'
import { useNavigate } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import apiClient from '@/api/client'
import { getPageItems } from '@/api/page'
import { Button } from '@/components/Button'

type PortRange = [number, number]
type Capabilities = { carriers?: string[] }
type NodeChoice = {
  id: string
  name: string
  region: string
  public_host: string
  capabilities: Capabilities
  protocols: string[]
  carrier_ports: Record<string, { port: number; enabled: boolean }>
  tcp_port_ranges: PortRange[]
  udp_port_ranges: PortRange[]
  http_shared_port: number
  https_shared_port: number
}
type ClientChoice = { id: string; name: string; online: boolean; capabilities: Capabilities }
type CertificateChoice = { id: string; domains: string[]; not_after: string }
type Subscription = {
  speed_limit_mbps: number
  max_conns_per_tunnel: number
  max_new_conns_per_sec: number
  allowed_protocols: string[]
  allow_custom_port: boolean
}

function isIpHost(value: string) {
  const host = value.trim().replace(/^\[|\]$/g, '')
  if (host.includes(':')) return /^[0-9a-f:]+$/i.test(host)
  const parts = host.split('.')
  return parts.length === 4 && parts.every((part) => /^\d{1,3}$/.test(part) && Number(part) <= 255)
}

type TunnelForm = {
  name: string
  node_id: string
  client_id: string
  carrier: string
  protocol: string
  port_mode: 'auto' | 'custom'
  remote_port: string
  target_host: string
  target_port: string
  target_url: string
  http_access: 'shared' | 'dedicated'
  domains: string
  https_enabled: boolean
  cert_id: string
  host_rewrite: string
  backend_tls_insecure: boolean
}

const emptyForm: TunnelForm = {
  name: '', node_id: '', client_id: '', carrier: '', protocol: '', port_mode: 'auto', remote_port: '',
  target_host: '', target_port: '', target_url: '', http_access: 'shared', domains: '', https_enabled: false,
  cert_id: '', host_rewrite: '$http_host', backend_tls_insecure: false,
}

export default function TunnelCreatePage() {
  const navigate = useNavigate()
  const [form, setForm] = useState(emptyForm)
  const [message, setMessage] = useState('')
  const [saving, setSaving] = useState(false)
  const [portCheck, setPortCheck] = useState<{ available: boolean; reason?: string } | null>(null)
  const [certificateFile, setCertificateFile] = useState<File | null>(null)
  const [privateKeyFile, setPrivateKeyFile] = useState<File | null>(null)
  const [uploadingCertificate, setUploadingCertificate] = useState(false)
  const subscription = useQuery({ queryKey: ['subscription'], queryFn: async () => (await apiClient.get('/v1/subscription')).data as Subscription | null })
  const nodes = useQuery({ queryKey: ['available-nodes'], queryFn: () => getPageItems<NodeChoice>('/v1/nodes') })
  const clients = useQuery({ queryKey: ['clients'], queryFn: () => getPageItems<ClientChoice>('/v1/clients') })
  const certificates = useQuery({ queryKey: ['certificates'], queryFn: () => getPageItems<CertificateChoice>('/v1/certificates') })
  const node = nodes.data?.find((item) => item.id === form.node_id)
  const client = clients.data?.find((item) => item.id === form.client_id)
  const protocols = useMemo(() => {
    const allowed = subscription.data?.allowed_protocols ?? []
    if (!node?.protocols) return allowed
    return allowed.filter((item) => node.protocols.includes(item))
  }, [node, subscription.data])
  const carriers = useMemo(() => {
    if (!node || !client) return []
    const nodeCarriers = new Set(node.capabilities.carriers ?? [])
    return (client.capabilities.carriers ?? []).filter((carrier) => nodeCarriers.has(carrier) && node.carrier_ports[carrier]?.enabled)
  }, [client, node])
  const protocol = form.protocol
  const isHttp = protocol === 'http'
  const targetIsHttps = isHttp && form.target_url.trim().toLowerCase().startsWith('https://')
  const needsPublicPort = Boolean(protocol) && (protocol !== 'http' || form.http_access === 'dedicated')
  const ranges = protocol === 'udp' ? node?.udp_port_ranges : node?.tcp_port_ranges
  const portLayer = protocol === 'udp' ? 'udp' : 'tcp'
  const portText = ranges?.map(([start, end]) => start === end ? String(start) : `${start}-${end}`).join(', ') ?? ''

  useEffect(() => {
    setForm((current) => ({ ...current, carrier: carriers.includes(current.carrier) ? current.carrier : carriers[0] ?? '' }))
  }, [carriers])

  useEffect(() => {
    setForm((current) => ({ ...current, protocol: protocols.includes(current.protocol) ? current.protocol : protocols[0] ?? '' }))
  }, [protocols])

  useEffect(() => {
    setPortCheck(null)
    if (!node || !form.remote_port || form.port_mode !== 'custom' || !needsPublicPort) return
    const port = Number(form.remote_port)
    if (!Number.isInteger(port) || port < 1 || port > 65535) return
    let active = true
    const timer = window.setTimeout(() => {
      apiClient.get(`/v1/nodes/${node.id}/ports/check`, { params: { l4: portLayer, port } })
        .then((response) => { if (active) setPortCheck(response.data) })
        .catch((error: unknown) => { if (active) setPortCheck({ available: false, reason: errorText(error) }) })
    }, 250)
    return () => { active = false; window.clearTimeout(timer) }
  }, [form.remote_port, form.port_mode, needsPublicPort, node, portLayer])

  function update<K extends keyof TunnelForm>(key: K, value: TunnelForm[K]) {
    setForm((current) => ({ ...current, [key]: value }))
    setMessage('')
  }

  async function chooseRandomPort() {
    if (!node || !needsPublicPort) return
    try {
      const response = await apiClient.post(`/v1/nodes/${node.id}/ports/random`, { l4: portLayer })
      update('remote_port', String(response.data.port))
    } catch (error) {
      setMessage(errorText(error))
    }
  }

  async function uploadCertificate() {
    if (!certificateFile || !privateKeyFile) {
      setMessage('请选择证书和对应私钥文件')
      return
    }
    setUploadingCertificate(true)
    try {
      const response = await apiClient.post('/v1/certificates', {
        certificate_pem: await certificateFile.text(),
        private_key_pem: await privateKeyFile.text(),
      })
      update('cert_id', response.data.id)
      await certificates.refetch()
      setCertificateFile(null)
      setPrivateKeyFile(null)
      setMessage('证书已验证并保存')
    } catch (error) {
      setMessage(errorText(error))
    } finally {
      setUploadingCertificate(false)
    }
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!subscription.data || !node || !client) {
      setMessage('订阅、节点或 Client 不可用')
      return
    }
    if (!carriers.includes(form.carrier)) {
      setMessage('所选 Carrier 不受当前节点和 Client 支持')
      return
    }
    if (form.port_mode === 'custom' && needsPublicPort && !portCheck?.available) {
      setMessage('请填写一个可用的公网端口')
      return
    }
    const domains = form.domains.split(/[\s,，]+/).map((domain) => domain.trim()).filter(Boolean)
    if (isHttp && form.http_access === 'shared' && domains.some((domain) => isIpHost(domain))) {
      setMessage('共享入口不能填写 IP，请使用域名')
      return
    }
    setSaving(true)
    setMessage('')
    try {
      const response = await apiClient.post('/v1/tunnels', {
        name: form.name,
        node_id: form.node_id,
        client_id: form.client_id,
        carrier: form.carrier,
        protocol: form.protocol,
        remote_port: needsPublicPort && form.port_mode === 'custom' ? Number(form.remote_port) : null,
        target_host: isHttp ? null : form.target_host,
        target_port: isHttp ? null : Number(form.target_port),
        target_url: isHttp ? form.target_url : null,
        http_access: isHttp ? form.http_access : null,
        domains: isHttp ? domains : [],
        https_enabled: isHttp && form.https_enabled,
        cert_id: isHttp && form.https_enabled ? form.cert_id : null,
        host_rewrite: isHttp ? form.host_rewrite : '$http_host',
        backend_tls_insecure: targetIsHttps && form.backend_tls_insecure,
      })
      setMessage(`隧道已提交，当前状态：${response.data.status}`)
      window.setTimeout(() => navigate('/tunnels'), 900)
    } catch (error) {
      setMessage(errorText(error))
    } finally {
      setSaving(false)
    }
  }

  if (subscription.isPending || nodes.isPending || clients.isPending || certificates.isPending) {
    return <section className="page-container"><p className="text-sm text-muted-foreground">正在加载可用节点、Client 和订阅</p></section>
  }
  if (subscription.isError || nodes.isError || clients.isError || certificates.isError) {
    return <section className="page-container"><p className="text-sm text-apple-red" role="alert">{errorText(subscription.error ?? nodes.error ?? clients.error ?? certificates.error)}</p></section>
  }
  if (!subscription.data) {
    return <section className="page-container"><h1 className="page-title">创建隧道</h1><p className="mt-4 text-sm text-muted-foreground">当前账号没有有效订阅，暂时不能创建隧道。</p></section>
  }

  return (
    <section className="page-container">
      <header className="page-header"><h1 className="page-title">创建隧道</h1></header>
      <section className="page-card mb-5 grid gap-3 p-5 text-sm sm:grid-cols-3">
        <p>单隧道速度上限：{subscription.data.speed_limit_mbps} Mbps</p>
        <p>单隧道并发上限：{subscription.data.max_conns_per_tunnel}</p>
        <p>每秒新建连接上限：{subscription.data.max_new_conns_per_sec}</p>
      </section>
      <form className="page-card grid gap-6 p-6" onSubmit={submit}>
        <Field label="隧道名称"><input className="apple-input w-full" value={form.name} onChange={(event) => update('name', event.target.value)} maxLength={100} required /></Field>
        <div className="grid gap-4 md:grid-cols-2">
          <Field label="Server 节点"><select className="apple-input w-full" value={form.node_id} onChange={(event) => update('node_id', event.target.value)} required><option value="">选择在线节点</option>{nodes.data?.map((item) => <option key={item.id} value={item.id}>{item.name} {item.region} {item.public_host}</option>)}</select></Field>
          <Field label="我的 Client"><select className="apple-input w-full" value={form.client_id} onChange={(event) => update('client_id', event.target.value)} required><option value="">选择 Client</option>{clients.data?.map((item) => <option key={item.id} value={item.id}>{item.name} {item.online ? '在线' : '离线'}</option>)}</select></Field>
          <Field label="Carrier 协议"><select className="apple-input w-full" value={form.carrier} onChange={(event) => update('carrier', event.target.value)} required><option value="">选择 Carrier</option>{carriers.map((carrier) => <option key={carrier} value={carrier}>{carrier.toUpperCase()}</option>)}</select></Field>
          <Field label="上层协议"><select className="apple-input w-full" value={form.protocol} onChange={(event) => update('protocol', event.target.value)} required><option value="">选择协议</option>{protocols.map((item) => <option key={item} value={item}>{item.toUpperCase()}</option>)}</select></Field>
        </div>
        {needsPublicPort && <section className="grid gap-4 md:grid-cols-2">
          {subscription.data.allow_custom_port && <Field label="公网端口分配方式"><select className="apple-input w-full" value={form.port_mode} onChange={(event) => update('port_mode', event.target.value as TunnelForm['port_mode'])}><option value="auto">自动分配</option><option value="custom">自定义端口</option></select></Field>}
          {form.port_mode === 'custom' && subscription.data.allow_custom_port && <Field label={`公网端口（可分配范围：${portText || '加载中'}）`}><div className="flex gap-2"><input className="apple-input w-full" type="number" min="1" max="65535" value={form.remote_port} onChange={(event) => update('remote_port', event.target.value)} required /><Button type="button" variant="secondary" onClick={chooseRandomPort}>随机分配</Button></div><span className={`mt-1 block text-xs ${portCheck?.available ? 'text-apple-green' : 'text-muted-foreground'}`}>{portCheck ? portCheck.available ? '端口可用' : portCheck.reason ?? '端口不可用' : '输入端口后检查可用状态'}</span></Field>}
          {!subscription.data.allow_custom_port && <p className="self-center text-sm text-muted-foreground">公网端口由系统在该节点可用范围内自动分配</p>}
        </section>}
        {isHttp ? <>
          <div className="grid gap-4 md:grid-cols-2">
            <Field label="HTTP 入口"><select className="apple-input w-full" value={form.http_access} onChange={(event) => update('http_access', event.target.value as TunnelForm['http_access'])}><option value="shared">共享入口（按域名） {node?.http_shared_port}/{node?.https_shared_port}</option><option value="dedicated">独立端口</option></select></Field>
            <Field label="转发目标 URL"><input className="apple-input w-full" type="url" placeholder="http://" value={form.target_url} onChange={(event) => update('target_url', event.target.value)} required /></Field>
          </div>
          <Field label="域名（多个域名以逗号或空格分隔）"><textarea className="apple-input w-full min-h-20" value={form.domains} onChange={(event) => update('domains', event.target.value)} placeholder={form.http_access === 'shared' ? 'service.example.com' : 'service.example.com 或 10.0.0.8'} required={form.http_access === 'shared' || form.https_enabled} /><span className="text-xs text-muted-foreground">{form.http_access === 'shared' ? '共享入口只能填写域名，不能填写 IP。' : '独立端口可以填写域名或 IP。'}</span></Field>
          <div className="grid gap-4 md:grid-cols-2">
            <Field label="Host 改写"><input className="apple-input w-full" value={form.host_rewrite} onChange={(event) => update('host_rewrite', event.target.value)} placeholder="$http_host" /></Field>
            <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={form.https_enabled} onChange={(event) => update('https_enabled', event.target.checked)} />启用公网 HTTPS</label>
          </div>
          {form.https_enabled && <div className="grid gap-4 md:grid-cols-2">
            <div className="grid gap-3">
              <Field label="HTTPS 证书"><select className="apple-input w-full" value={form.cert_id} onChange={(event) => update('cert_id', event.target.value)} required><option value="">选择有效证书</option>{certificates.data?.map((certificate) => <option key={certificate.id} value={certificate.id}>{certificate.domains.join(', ')} {certificate.not_after}</option>)}</select></Field>
              <div className="grid gap-2 sm:grid-cols-2">
                <Field label="上传证书链 PEM"><input className="apple-input w-full" type="file" accept=".pem,.crt,.cer" onChange={(event) => setCertificateFile(event.target.files?.[0] ?? null)} /></Field>
                <Field label="上传私钥 PEM"><input className="apple-input w-full" type="file" accept=".pem,.key" onChange={(event) => setPrivateKeyFile(event.target.files?.[0] ?? null)} /></Field>
              </div>
              <Button type="button" variant="secondary" loading={uploadingCertificate} onClick={uploadCertificate}>验证并保存证书</Button>
            </div>
          </div>}
          {targetIsHttps && <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={form.backend_tls_insecure} onChange={(event) => update('backend_tls_insecure', event.target.checked)} />不校验上游证书（自签证书可用）</label>}
        </> : <div className="grid gap-4 md:grid-cols-2">
          <Field label="转发目标地址"><input className="apple-input w-full" value={form.target_host} onChange={(event) => update('target_host', event.target.value)} placeholder="127.0.0.1 或内网域名" required /></Field>
          <Field label="转发目标端口"><input className="apple-input w-full" type="number" min="1" max="65535" value={form.target_port} onChange={(event) => update('target_port', event.target.value)} required /></Field>
        </div>}
        {message && <p className="text-sm text-apple-red" role="status">{message}</p>}
        <div className="flex gap-3"><Button type="submit" loading={saving}>创建隧道</Button><Button type="button" variant="secondary" onClick={() => navigate('/tunnels')}>取消</Button></div>
      </form>
    </section>
  )
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return <label className="block"><span className="apple-label">{label}</span>{children}</label>
}

function errorText(error: unknown): string {
  if (typeof error === 'object' && error !== null && 'response' in error) {
    const response = (error as { response?: { data?: { error?: string } } }).response
    if (response?.data?.error) return response.data.error
  }
  return '无法连接 board'
}
