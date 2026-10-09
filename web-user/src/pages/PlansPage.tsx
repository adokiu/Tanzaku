import { useState } from 'react'
import { useMutation } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { usePagedList } from '@/api/page'
import { Button } from '@/components/Button'
import { PageListHeader } from '@/components/PageListHeader'
import { translateApiError } from '@/i18n/apiError'
import { toast } from '@/stores/toast'
import { formatBalanceCents } from '@/utils/formatMoney'

type Plan = {
  id: string
  name: string
  description: string
  traffic_period: string
  speed_limit_mbps: number
  traffic_quota_bytes: number | null
  price_month_cents: number | null
  price_quarter_cents: number | null
  price_half_year_cents: number | null
  price_year_cents: number | null
  price_two_year_cents: number | null
  price_three_year_cents: number | null
  price_traffic_pack_cents: number | null
  price_reset_pack_cents: number | null
}

const PERIODS = [
  ['month', 'price_month_cents'],
  ['quarter', 'price_quarter_cents'],
  ['half_year', 'price_half_year_cents'],
  ['year', 'price_year_cents'],
  ['two_year', 'price_two_year_cents'],
  ['three_year', 'price_three_year_cents'],
  ['traffic_pack', 'price_traffic_pack_cents'],
  ['reset_pack', 'price_reset_pack_cents'],
] as const

function amountFor(plan: Plan, period: (typeof PERIODS)[number][0]): number | null {
  const key = PERIODS.find(([item]) => item === period)?.[1]
  if (!key) return null
  return plan[key]
}

export default function PlansPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [busyKey, setBusyKey] = useState<string | null>(null)
  const plans = usePagedList<Plan>(['user-plans'], '/v1/plans')
  const purchase = useMutation({
    mutationFn: async ({ planId, period }: { planId: string; period: string }) => {
      setBusyKey(`${planId}:${period}`)
      const response = await apiClient.post('/v1/orders', { plan_id: planId, period })
      return response.data as { id: string }
    },
    onSuccess: (order) => {
      navigate(`/orders/${order.id}/pay`)
    },
    onError: (error) => toast.error(translateApiError(t, error)),
    onSettled: () => setBusyKey(null),
  })

  return (
    <section className="page-container">
      <PageListHeader title={t('pages.plans')} />
      <p className="mb-4 text-sm text-muted-foreground">{t('plans.hint')}</p>
      {plans.query.isError ? (
        <p className="page-card p-5 text-sm text-destructive">{translateApiError(t, plans.query.error)}</p>
      ) : (
        <div className="grid gap-4">
          {plans.items.map((plan) => {
            const offers = PERIODS.filter(([period]) => amountFor(plan, period) != null)
            const listPrice = plan.price_month_cents ?? (offers[0] ? amountFor(plan, offers[0][0]) : null)
            return (
              <article key={plan.id} className="page-card flex flex-col gap-4 p-5">
                <div className="flex flex-wrap items-start justify-between gap-4">
                  <div>
                    <h2 className="text-base font-semibold">{plan.name}</h2>
                    <p className="mt-1 text-sm text-muted-foreground">
                      {plan.speed_limit_mbps} Mbps
                      {plan.traffic_quota_bytes == null ? ` · ${t('plans.unlimited')}` : ''}
                      {` · ${t(`enums.billingPeriod.${plan.traffic_period}`)}`}
                    </p>
                    {plan.description ? <p className="mt-2 text-sm text-muted-foreground">{plan.description}</p> : null}
                  </div>
                  {listPrice != null ? (
                    <div className="text-right">
                      <div className="text-xs text-muted-foreground">{t('plans.basePrice')}</div>
                      <div className="text-lg font-semibold">{formatBalanceCents(listPrice)}</div>
                    </div>
                  ) : null}
                </div>
                {offers.length === 0 ? (
                  <p className="text-sm text-muted-foreground">{t('plans.noPrice')}</p>
                ) : (
                  <div className="flex flex-wrap gap-2">
                    {offers.map(([period]) => {
                      const amount = amountFor(plan, period)
                      if (amount == null) return null
                      const key = `${plan.id}:${period}`
                      return (
                        <Button
                          key={period}
                          size="sm"
                          variant="secondary"
                          loading={purchase.isPending && busyKey === key}
                          onClick={() => purchase.mutate({ planId: plan.id, period })}
                        >
                          {t(`plans.period.${period}`)} · {formatBalanceCents(amount)}
                        </Button>
                      )
                    })}
                  </div>
                )}
              </article>
            )
          })}
          {!plans.query.isPending && plans.items.length === 0 ? (
            <p className="page-card p-5 text-sm text-muted-foreground">{t('plans.empty')}</p>
          ) : null}
        </div>
      )}
    </section>
  )
}
