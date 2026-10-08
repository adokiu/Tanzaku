import { useEffect, useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useParams } from 'react-router-dom'
import apiClient from '@/api/client'
import { Button } from '@/components/Button'

type TunnelDetail = {
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
  http_access?: string | null
  https_enabled: boolean
  status: string
  enabled: boolean
  speed_limit_mbps: number
  max_conns: number
  max_new_conns_per_sec: number
  last_error?: string | null
  domains: string[]
}

type LiveTunnelStats = {
  status: string
  bytes_in: number
  bytes_out: number
  active_conns: number
  rejects_quota: number
  rejects_guard: number
}

type LivePayload = {
  ok: boolean
  degraded?: boolean
  tunnels?: Record<string, LiveTunnelStats>
}

export default function TunnelDetailPage() {
  const { tunnelId } = useParams<{ tunnelId: string }>()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [live, setLive] = useState<LiveTunnelStats | null>(null)
  const [liveError, setLiveError] = useState('')
  const [message, setMessage] = useState('')

  const query = useQuery({
    queryKey: ['user-tunnel', tunnelId],
    enabled: Boolean(tunnelId),
    queryFn: async () => (await apiClient.get(`/v1/tunnels/${tunnelId}`)).data as TunnelDetail,
  })

  useEffect(() => {
    if (!tunnelId) return
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
    const socket = new WebSocket(`${protocol}//${window.location.host}/api/v1/live`)
    socket.onmessage = (event) => {
      try {
        const payload = JSON.parse(String(event.data)) as LivePayload
        if (!payload.ok || !payload.tunnels) {
          setLiveError('实时数据不可用')
          return
        }
        const stats = payload.tunnels[tunnelId]
        if (stats) {
          setLive(stats)
          setLiveError('')
        }
      } catch {
        setLiveError('实时数据解析失败')
      }
    }
    socket.onerror = () => setLiveError('无法连接实时通道')
    return () => socket.close()
  }, [tunnelId])

  const suspend = useMutation({
    mutationFn: () => apiClient.post(`/v1/tunnels/${tunnelId}/suspend`),
    onSuccess: async () => {
      setMessage('隧道已暂停')
      await queryClient.invalidateQueries({ queryKey: ['user-tunnel', tunnelId] })
      await queryClient.invalidateQueries({ queryKey: ['user-tunnels'] })
    },
    onError: (error) => setMessage(apiError(error)),
  })

  const closeTunnel = useMutation({
    mutationFn: () => apiClient.post(`/v1/tunnels/${tunnelId}/close`),
    onSuccess: async () => {
      setMessage('隧道已关闭，node/client 将自动停止转发')
      await queryClient.invalidateQueries({ queryKey: ['user-tunnel', tunnelId] })
      await queryClient.invalidateQueries({ queryKey: ['user-tunnels'] })
    },
    onError: (error) => setMessage(apiError(error)),
  })

  const openTunnel = useMutation({
    mutationFn: () => apiClient.post(`/v1/tunnels/${tunnelId}/open`),
    onSuccess: async () => {
      setMessage('隧道已开启，node/client 将自动恢复')
      await queryClient.invalidateQueries({ queryKey: ['user-tunnel', tunnelId] })
      await queryClient.invalidateQueries({ queryKey: ['user-tunnels'] })
    },
    onError: (error) => setMessage(apiError(error)),
  })

  const resume = useMutation({
    mutationFn: () => apiClient.post(`/v1/tunnels/${tunnelId}/resume`),
    onSuccess: async () => {
      setMessage('隧道已恢复')
      await queryClient.invalidateQueries({ queryKey: ['user-tunnel', tunnelId] })
      await queryClient.invalidateQueries({ queryKey: ['user-tunnels'] })
    },
    onError: (error) => setMessage(apiError(error)),
  })

  const tunnel = query.data
  const targetLabel = useMemo(() => {
    if (!tunnel) return ''
    if (tunnel.target_url) return tunnel.target_url
    if (tunnel.target_host && tunnel.target_port) return `${tunnel.target_host}:${tunnel.target_port}`
    return '—'
  }, [tunnel])

  if (!tunnelId) {
    return <section className="page-container"><p className="text-sm text-apple-red">缺少隧道 ID</p></section>
  }

  if (query.isPending) {
    return <section className="page-container"><p className="text-sm text-muted-foreground">正在加载隧道详情</p></section>
  }

  if (query.isError || !tunnel) {
    return (
      <section className="page-container">
        <p className="text-sm text-apple-red" role="alert">{apiError(query.error)}</p>
        <Button type="button" variant="secondary" className="mt-4" onClick={() => navigate('/tunnels')}>返回列表</Button>
      </section>
    )
  }

  const canClose = tunnel.enabled && tunnel.status !== 'suspended' && tunnel.status !== 'pending_review'
  const canOpen = !tunnel.enabled && tunnel.status !== 'suspended' && tunnel.status !== 'pending_review'
  const canSuspend = tunnel.enabled && tunnel.status !== 'suspended' && tunnel.status !== 'pending_review'
  const canResume = tunnel.status === 'suspended'

  return (
    <section className="page-container">
      <header className="page-header flex flex-wrap items-center justify-between gap-4">
        <div>
          <p className="text-sm text-muted-foreground"><Link to="/tunnels">我的隧道</Link> / 详情</p>
          <h1 className="page-title">{tunnel.name}</h1>
        </div>
        <div className="flex flex-wrap gap-2">
          {canClose && (
            <Button type="button" variant="secondary" loading={closeTunnel.isPending} onClick={() => closeTunnel.mutate()}>关闭</Button>
          )}
          {canOpen && (
            <Button type="button" loading={openTunnel.isPending} onClick={() => openTunnel.mutate()}>开启</Button>
          )}
          {canSuspend && (
            <Button type="button" variant="secondary" loading={suspend.isPending} onClick={() => suspend.mutate()}>暂停（订阅）</Button>
          )}
          {canResume && (
            <Button type="button" loading={resume.isPending} onClick={() => resume.mutate()}>恢复（订阅）</Button>
          )}
          <Button type="button" variant="secondary" onClick={() => navigate('/tunnels')}>返回列表</Button>
        </div>
      </header>

      {message && <p className="mb-4 text-sm text-muted-foreground" role="status">{message}</p>}

      <div className="grid gap-4 lg:grid-cols-2">
        <article className="page-card p-5 space-y-3">
          <h2 className="font-medium">配置</h2>
          <dl className="grid grid-cols-2 gap-x-4 gap-y-2 text-sm">
            <dt className="text-muted-foreground">状态</dt><dd>{tunnel.status}{tunnel.enabled ? '' : '（未启用）'}</dd>
            <dt className="text-muted-foreground">节点</dt><dd>{tunnel.node_name ?? tunnel.node_id}</dd>
            <dt className="text-muted-foreground">Client</dt><dd>{tunnel.client_name ?? tunnel.client_id}</dd>
            <dt className="text-muted-foreground">协议</dt><dd>{tunnel.carrier} / {tunnel.protocol}</dd>
            <dt className="text-muted-foreground">公网端口</dt><dd>{tunnel.remote_port ?? '共享入口'}</dd>
            <dt className="text-muted-foreground">后端</dt><dd className="col-span-1 break-all">{targetLabel}</dd>
            {tunnel.http_access && (<><dt className="text-muted-foreground">HTTP 接入</dt><dd>{tunnel.http_access}</dd></>)}
            {tunnel.domains.length > 0 && (<><dt className="text-muted-foreground">域名</dt><dd>{tunnel.domains.join(', ')}</dd></>)}
            <dt className="text-muted-foreground">限速</dt><dd>{tunnel.speed_limit_mbps} Mbps</dd>
            <dt className="text-muted-foreground">并发 / 新建</dt><dd>{tunnel.max_conns} / {tunnel.max_new_conns_per_sec} 每秒</dd>
          </dl>
          {tunnel.last_error && <p className="text-sm text-apple-red" role="alert">{tunnel.last_error}</p>}
        </article>

        <article className="page-card p-5 space-y-3">
          <h2 className="font-medium">实时流量</h2>
          {liveError && <p className="text-sm text-muted-foreground">{liveError}</p>}
          {live ? (
            <dl className="grid grid-cols-2 gap-x-4 gap-y-2 text-sm">
              <dt className="text-muted-foreground">入站字节</dt><dd>{formatBytes(live.bytes_in)}</dd>
              <dt className="text-muted-foreground">出站字节</dt><dd>{formatBytes(live.bytes_out)}</dd>
              <dt className="text-muted-foreground">活跃连接</dt><dd>{live.active_conns}</dd>
              <dt className="text-muted-foreground">配额拒绝</dt><dd>{live.rejects_quota}</dd>
              <dt className="text-muted-foreground">防护拒绝</dt><dd>{live.rejects_guard}</dd>
              <dt className="text-muted-foreground">上报状态</dt><dd>{live.status}</dd>
            </dl>
          ) : (
            <p className="text-sm text-muted-foreground">等待实时数据…</p>
          )}
        </article>
      </div>
    </section>
  )
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`
  if (value < 1024 * 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(2)} MiB`
  return `${(value / (1024 * 1024 * 1024)).toFixed(2)} GiB`
}

function apiError(error: unknown): string {
  if (typeof error === 'object' && error !== null && 'response' in error) {
    const response = (error as { response?: { data?: { error?: string } } }).response
    if (response?.data?.error) return response.data.error
  }
  return '无法连接 board'
}
