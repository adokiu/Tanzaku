import { useState, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import apiClient from '@/api/client'
import { PageListHeader } from '@/components/PageListHeader'
import { translateApiError } from '@/i18n/apiError'
import { formatDateTime, formatDurationSecs } from '@/utils/formatDateTime'
import { formatBytes, formatNetSpeed } from '@/utils/formatMetrics'
import { translateGuardRule } from './security/policyShared'
import './DashboardPage.css'

type TrafficPoint = { at: number; bytes_in: number; bytes_out: number }

type Dashboard = {
  users: number
  active_subscriptions: number
  nodes: number
  nodes_online: number
  clients: number
  clients_online: number
  tunnels: number
  tunnels_active: number
  connections_tcp: number
  connections_udp: number
  traffic_today_bytes: number
  traffic_24h_bytes: number
  traffic_month_bytes: number
  bandwidth_in_bps: number
  bandwidth_out_bps: number
  hourly: TrafficPoint[]
  daily: TrafficPoint[]
  protocols: { protocol: string; count: number }[]
  nodes_load: { name: string; connections_tcp: number; connections_udp: number }[]
  events: {
    id: number
    node_name: string
    rule: string
    tunnel_id: string | null
    intensity: number
    duration_secs: number
    last_seen_at: string
  }[]
}

const CHART_W = 720
const CHART_H = 228
const PAD = { l: 58, r: 10, t: 12, b: 26 }

export default function DashboardPage() {
  const { t, i18n } = useTranslation()
  const query = useQuery({
    queryKey: ['admin-dashboard'],
    queryFn: async () => (await apiClient.get('/v1/admin/dashboard')).data as Dashboard,
    refetchInterval: 10_000,
    placeholderData: (previous) => previous,
  })
  const data = query.data
  const count = (value: number) => new Intl.NumberFormat(i18n.language).format(value)

  return (
    <section className="page-container">
      <PageListHeader />
      {query.isError ? (
        <p className="page-card p-5 text-sm text-apple-red" role="alert">{translateApiError(t, query.error)}</p>
      ) : !data ? (
        <div className="dash-grid dash-grid--stats">
          {Array.from({ length: 8 }, (_, index) => <div className="dash-skeleton" key={index} />)}
        </div>
      ) : (
        <div className="dash">
          <div className="dash-grid dash-grid--stats">
            <StatCard
              label={t('dashboard.servers')}
              value={`${count(data.nodes_online)} / ${count(data.nodes)}`}
              hint={t('dashboard.onlineRatio', { percent: data.nodes === 0 ? 0 : Math.round((data.nodes_online / data.nodes) * 100) })}
              ratio={data.nodes === 0 ? 0 : data.nodes_online / data.nodes}
            />
            <StatCard
              label={t('dashboard.clients')}
              value={`${count(data.clients_online)} / ${count(data.clients)}`}
              hint={t('dashboard.onlineRatio', { percent: data.clients === 0 ? 0 : Math.round((data.clients_online / data.clients) * 100) })}
              ratio={data.clients === 0 ? 0 : data.clients_online / data.clients}
            />
            <StatCard
              label={t('dashboard.tunnels')}
              value={count(data.tunnels)}
              hint={t('dashboard.running', { n: count(data.tunnels_active) })}
              ratio={data.tunnels === 0 ? 0 : data.tunnels_active / data.tunnels}
            />
            <StatCard
              label={t('dashboard.connections')}
              value={count(data.connections_tcp + data.connections_udp)}
              hint={t('dashboard.tcpUdp', { tcp: count(data.connections_tcp), udp: count(data.connections_udp) })}
            />
            <StatCard label={t('dashboard.users')} value={count(data.users)} />
            <StatCard label={t('dashboard.subscriptions')} value={count(data.active_subscriptions)} />
            <StatCard label={t('dashboard.today')} value={formatBytes(data.traffic_today_bytes)} />
            <StatCard label={t('dashboard.last24h')} value={formatBytes(data.traffic_24h_bytes)} />
            <StatCard label={t('dashboard.month')} value={formatBytes(data.traffic_month_bytes)} />
            <StatCard
              label={t('dashboard.bandwidth')}
              value={formatNetSpeed(data.bandwidth_in_bps + data.bandwidth_out_bps)}
              hint={t('dashboard.bandwidthDetail', {
                in: formatNetSpeed(data.bandwidth_in_bps),
                out: formatNetSpeed(data.bandwidth_out_bps),
              })}
            />
          </div>

          <div className="dash-grid dash-grid--charts">
            <ChartPanel title={t('dashboard.hourlyTitle')}>
              <SeriesChart points={data.hourly} mode="line" labelEvery={4} formatLabel={hourLabel} />
            </ChartPanel>
            <ChartPanel title={t('dashboard.dailyTitle')}>
              <SeriesChart points={data.daily} mode="bar" labelEvery={1} formatLabel={dayLabel} />
            </ChartPanel>
          </div>

          <div className="dash-grid dash-grid--split">
            <section className="dash-panel">
              <div className="dash-panel__head">
                <h2 className="dash-panel__title">{t('dashboard.protocolTitle')}</h2>
              </div>
              {data.protocols.length === 0 ? (
                <p className="dash-empty">{t('dashboard.protocolsEmpty')}</p>
              ) : (
                <HBars
                  rows={data.protocols.map((item) => ({
                    label: item.protocol.toUpperCase(),
                    segments: [{ value: item.count, className: 'dash-hbars__seg dash-seg--out' }],
                    value: count(item.count),
                  }))}
                />
              )}
            </section>
            <section className="dash-panel">
              <div className="dash-panel__head">
                <h2 className="dash-panel__title">{t('dashboard.nodeLoadTitle')}</h2>
                <div className="dash-legend">
                  <span><i className="is-in" />TCP</span>
                  <span><i className="is-out" />UDP</span>
                </div>
              </div>
              {data.nodes_load.length === 0 ? (
                <p className="dash-empty">{t('dashboard.nodesEmpty')}</p>
              ) : (
                <HBars
                  rows={data.nodes_load.map((item) => ({
                    label: item.name,
                    segments: [
                      { value: item.connections_tcp, className: 'dash-hbars__seg dash-seg--tcp' },
                      { value: item.connections_udp, className: 'dash-hbars__seg dash-seg--udp' },
                    ],
                    value: count(item.connections_tcp + item.connections_udp),
                  }))}
                />
              )}
            </section>
          </div>

          <section className="dash-panel">
            <div className="dash-panel__head">
              <h2 className="dash-panel__title">{t('dashboard.eventsTitle')}</h2>
              <Link className="dash-panel__link" to="/security/events">{t('dashboard.eventsMore')}</Link>
            </div>
            {data.events.length === 0 ? (
              <p className="dash-empty">{t('dashboard.eventsEmpty')}</p>
            ) : (
              <ul className="dash-events">
                {data.events.map((event) => (
                  <li key={event.id}>
                    <span className="dash-event__rule">{translateGuardRule(t, event.rule)}</span>
                    <span className="dash-event__meta">
                      {[
                        event.node_name,
                        t('dashboard.intensity', { n: count(event.intensity) }),
                        formatDurationSecs(event.duration_secs),
                        formatDateTime(event.last_seen_at),
                      ]
                        .filter(Boolean)
                        .map((item) => (
                          <span key={String(item)} className="dash-event__tag">{item}</span>
                        ))}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </section>
        </div>
      )}
    </section>
  )
}

function StatCard({ label, value, hint, ratio }: { label: string; value: string; hint?: string; ratio?: number }) {
  return (
    <article className="dash-card">
      <div className="dash-card__top">
        <span className="dash-card__label">{label}</span>
        {ratio == null ? null : <Ring ratio={ratio} />}
      </div>
      <div>
        <p className="dash-card__value">{value}</p>
        {hint ? <p className="dash-card__hint">{hint}</p> : null}
      </div>
    </article>
  )
}

function Ring({ ratio }: { ratio: number }) {
  const radius = 16
  const length = 2 * Math.PI * radius
  const clamped = Math.max(0, Math.min(1, ratio))
  return (
    <svg className="dash-ring" width="40" height="40" viewBox="0 0 40 40" aria-hidden="true">
      <circle className="dash-ring__track" cx="20" cy="20" r={radius} />
      <circle
        className="dash-ring__value"
        cx="20"
        cy="20"
        r={radius}
        strokeDasharray={`${length * clamped} ${length}`}
        transform="rotate(-90 20 20)"
      />
    </svg>
  )
}

function ChartPanel({ title, children }: { title: string; children: ReactNode }) {
  const { t } = useTranslation()
  return (
    <section className="dash-panel">
      <div className="dash-panel__head">
        <h2 className="dash-panel__title">{title}</h2>
        <div className="dash-legend">
          <span><i className="is-in" />{t('dashboard.inbound')}</span>
          <span><i className="is-out" />{t('dashboard.outbound')}</span>
        </div>
      </div>
      {children}
    </section>
  )
}

function SeriesChart({
  points,
  mode,
  labelEvery,
  formatLabel,
}: {
  points: TrafficPoint[]
  mode: 'line' | 'bar'
  labelEvery: number
  formatLabel: (at: number) => string
}) {
  const [hover, setHover] = useState<number | null>(null)
  const innerW = CHART_W - PAD.l - PAD.r
  const innerH = CHART_H - PAD.t - PAD.b
  const max = Math.max(1, ...points.flatMap((point) => [point.bytes_in, point.bytes_out]))
  const xAt = (index: number) => {
    if (points.length <= 1) return PAD.l + innerW / 2
    return PAD.l + (index / (points.length - 1)) * innerW
  }
  const yAt = (value: number) => PAD.t + innerH - (value / max) * innerH
  const line = (key: 'bytes_in' | 'bytes_out') => points
    .map((point, index) => `${index === 0 ? 'M' : 'L'}${xAt(index).toFixed(1)},${yAt(point[key]).toFixed(1)}`)
    .join(' ')
  const area = (key: 'bytes_in' | 'bytes_out') => {
    if (points.length === 0) return ''
    const base = (PAD.t + innerH).toFixed(1)
    return `${line(key)} L${xAt(points.length - 1).toFixed(1)},${base} L${xAt(0).toFixed(1)},${base} Z`
  }
  const ticks = [0, 0.5, 1]
  const active = hover != null ? points[hover] : null
  const groupW = points.length === 0 ? innerW : innerW / points.length

  return (
    <>
      <svg
        className="dash-chart"
        viewBox={`0 0 ${CHART_W} ${CHART_H}`}
        role="img"
        onMouseLeave={() => setHover(null)}
        onMouseMove={(event) => {
          if (points.length === 0) return
          const rect = event.currentTarget.getBoundingClientRect()
          const vx = ((event.clientX - rect.left) / rect.width) * CHART_W
          if (mode === 'bar') {
            const index = Math.floor((vx - PAD.l) / groupW)
            setHover(index >= 0 && index < points.length ? index : null)
            return
          }
          const ratio = (vx - PAD.l) / innerW
          const index = Math.round(ratio * (points.length - 1))
          setHover(index >= 0 && index < points.length ? index : null)
        }}
      >
        {ticks.map((tick) => {
          const y = yAt(max * tick)
          return (
            <g key={tick}>
              <line className="dash-chart__grid" x1={PAD.l} x2={CHART_W - PAD.r} y1={y} y2={y} />
              <text className="dash-chart__label" x={PAD.l - 8} y={y + 4} textAnchor="end">
                {formatBytes(max * tick)}
              </text>
            </g>
          )
        })}
        {mode === 'line' ? (
          <>
            <path className="dash-chart__area-out" d={area('bytes_out')} />
            <path className="dash-chart__area-in" d={area('bytes_in')} />
            <path className="dash-chart__line-out" d={line('bytes_out')} />
            <path className="dash-chart__line-in" d={line('bytes_in')} />
          </>
        ) : points.map((point, index) => {
          const slot = PAD.l + index * groupW
          const barW = Math.max(3, groupW * 0.28)
          const inH = (point.bytes_in / max) * innerH
          const outH = (point.bytes_out / max) * innerH
          const base = PAD.t + innerH
          return (
            <g key={point.at}>
              <rect className="dash-chart__bar-in" x={slot + groupW * 0.18} y={base - inH} width={barW} height={inH} rx="2" />
              <rect className="dash-chart__bar-out" x={slot + groupW * 0.18 + barW + 2} y={base - outH} width={barW} height={outH} rx="2" />
            </g>
          )
        })}
        {points.map((point, index) => (
          index % labelEvery === 0 ? (
            <text key={point.at} className="dash-chart__label" x={mode === 'bar' ? PAD.l + index * groupW + groupW / 2 : xAt(index)} y={CHART_H - 6} textAnchor="middle">
              {formatLabel(point.at)}
            </text>
          ) : null
        ))}
        {hover != null && mode === 'line' ? (
          <line className="dash-chart__guide" x1={xAt(hover)} x2={xAt(hover)} y1={PAD.t} y2={PAD.t + innerH} />
        ) : null}
      </svg>
      <p className="dash-tip">
        {active
          ? `${formatLabel(active.at)}  ${formatBytes(active.bytes_in)} / ${formatBytes(active.bytes_out)}`
          : ' '}
      </p>
    </>
  )
}

function HBars({ rows }: { rows: { label: string; value: string; segments: { value: number; className: string }[] }[] }) {
  const max = Math.max(1, ...rows.map((row) => row.segments.reduce((sum, segment) => sum + segment.value, 0)))
  return (
    <ul className="dash-hbars">
      {rows.map((row) => (
        <li key={row.label}>
          <span className="dash-hbars__label" title={row.label}>{row.label}</span>
          <span className="dash-hbars__track">
            {row.segments.map((segment) => (
              <span
                key={segment.className}
                className={segment.className}
                style={{ width: `${(segment.value / max) * 100}%` }}
              />
            ))}
          </span>
          <span className="dash-hbars__value">{row.value}</span>
        </li>
      ))}
    </ul>
  )
}

function hourLabel(at: number) {
  const date = new Date(at * 1000)
  return `${String(date.getHours()).padStart(2, '0')}:00`
}

function dayLabel(at: number) {
  const date = new Date(at * 1000)
  return `${date.getMonth() + 1}/${date.getDate()}`
}
