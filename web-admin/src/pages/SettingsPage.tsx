import { useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { Button } from '@/components/Button'
import { Input } from '@/components/Input'
import { PageListHeader } from '@/components/PageListHeader'
import { translateApiError } from '@/i18n/apiError'
import { formatDateTime } from '@/utils/formatDateTime'

type Setting = { key: string; value: unknown; updated_at: string }

const CLIENT_IP_GEO_PROVIDERS = ['ipinfo', 'ip9', 'ip_sb'] as const
type ClientIpGeoProvider = (typeof CLIENT_IP_GEO_PROVIDERS)[number]

function isClientIpGeoProvider(value: unknown): value is ClientIpGeoProvider {
  return typeof value === 'string' && CLIENT_IP_GEO_PROVIDERS.includes(value as ClientIpGeoProvider)
}

export default function SettingsPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const query = useQuery({
    queryKey: ['admin-settings'],
    queryFn: async () => (await apiClient.get('/v1/admin/settings')).data as Setting[],
  })
  const update = useMutation({
    mutationFn: ({ key, value }: { key: string; value: unknown }) =>
      apiClient.post('/v1/admin/settings', { key, value }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ['admin-settings'] })
    },
  })
  const metricsDraft = useMemo(() => {
    const row = query.data?.find((item) => item.key === 'node_host_metrics_interval_secs')
    if (typeof row?.value === 'number') return String(row.value)
    return '1'
  }, [query.data])
  const [metricsInterval, setMetricsInterval] = useState('')
  const metricsValue = metricsInterval || metricsDraft

  return <section className="page-container">
    <PageListHeader
      title={t('pages.settings')}
      actions={(
        <Button variant="secondary" size="sm" onClick={() => query.refetch()} disabled={query.isFetching}>
          <RefreshCw size={16} className={query.isFetching ? 'animate-spin' : ''} />
          {t('common.refresh')}
        </Button>
      )}
    />
    {query.isError ? <p className="page-card p-5 text-sm text-apple-red" role="alert">{translateApiError(t, query.error)}</p> : (
      <div className="grid gap-3">
        {(query.data ?? []).map((setting) => (
          <div className="page-card flex items-center justify-between gap-4 p-5" key={setting.key}>
            <div>
              <p className="font-medium">{t(`settings.${setting.key}`, { defaultValue: setting.key })}</p>
              <p className="mt-1 text-xs text-muted-foreground">{setting.key} · {formatDateTime(setting.updated_at)}</p>
            </div>
            {setting.key === 'client_ip_geo_provider' ? (
              <div className="flex items-center gap-2">
                <select
                  className="apple-input min-w-[12rem]"
                  value={isClientIpGeoProvider(setting.value) ? setting.value : 'ipinfo'}
                  onChange={(event) => {
                    const value = event.target.value
                    if (!isClientIpGeoProvider(value)) return
                    update.mutate({ key: setting.key, value })
                  }}
                  disabled={update.isPending}
                >
                  {CLIENT_IP_GEO_PROVIDERS.map((provider) => (
                    <option key={provider} value={provider}>
                      {t(`settingsGeoProvider.${provider}`)}
                    </option>
                  ))}
                </select>
              </div>
            ) : setting.key === 'node_host_metrics_interval_secs' ? (
              <div className="flex items-center gap-2">
                <Input
                  type="number"
                  min={1}
                  max={3600}
                  className="w-24 font-number"
                  value={metricsValue}
                  onChange={(event) => setMetricsInterval(event.target.value)}
                />
                <span className="text-sm text-muted-foreground">{t('settings.secondsUnit')}</span>
                <Button
                  type="button"
                  size="sm"
                  loading={update.isPending}
                  onClick={() => {
                    const parsed = Number(metricsValue)
                    if (!Number.isFinite(parsed) || parsed < 1 || parsed > 3600) return
                    update.mutate({ key: setting.key, value: parsed })
                    setMetricsInterval('')
                  }}
                >
                  {t('common.save')}
                </Button>
              </div>
            ) : typeof setting.value === 'boolean' ? (
              <Button
                type="button"
                variant={setting.value ? 'primary' : 'secondary'}
                loading={update.isPending}
                onClick={() => update.mutate({ key: setting.key, value: !setting.value })}
              >
                {setting.value ? t('common.enabled') : t('common.disabled')}
              </Button>
            ) : (
              <code className="text-xs">{JSON.stringify(setting.value)}</code>
            )}
          </div>
        ))}
      </div>
    )}
  </section>
}
