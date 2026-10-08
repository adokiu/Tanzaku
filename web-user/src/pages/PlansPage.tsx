import { useState } from 'react'
import { useMutation } from '@tanstack/react-query'
import apiClient from '@/api/client'
import { usePagedList } from '@/api/page'
import { Button } from '@/components/Button'
import { ListPager } from '@/components/DataTable'

type Plan = {
  id: string
  name: string
  description: string
  traffic_period: string
  speed_limit_mbps: number
  traffic_quota_bytes: number | null
}

const periodLabel: Record<string, string> = {
  day: '日付',
  week: '周付',
  month: '月付',
  quarter: '季付',
  year: '年付',
  lifetime: '一次性',
}

export default function PlansPage() {
  const [message, setMessage] = useState('')
  const plans = usePagedList<Plan>(['user-plans'], '/v1/plans')
  const purchase = useMutation({
    mutationFn: async (planId: string) => {
      await apiClient.post('/v1/orders', { plan_id: planId })
    },
    onSuccess: () => setMessage('订单已完成，订阅已开通。'),
    onError: (error) => setMessage(apiError(error)),
  })

  return (
    <section className="page-container">
      <header className="page-header">
        <h1 className="page-title">购买套餐</h1>
      </header>
      <p className="mb-4 text-sm text-muted-foreground">购买或更换套餐会生成订单。已有订阅时按升级处理。</p>
      {message ? <p className="mb-4 text-sm text-muted-foreground" role="status">{message}</p> : null}
      {plans.query.isError ? (
        <p className="page-card p-5 text-sm text-apple-red" role="alert">{apiError(plans.query.error)}</p>
      ) : (
        <div className="grid gap-4">
          {plans.items.map((plan) => (
            <article key={plan.id} className="page-card flex flex-wrap items-center justify-between gap-4 p-5">
              <div>
                <h2 className="text-base font-semibold">{plan.name}</h2>
                <p className="mt-1 text-sm text-muted-foreground">
                  {periodLabel[plan.traffic_period] ?? plan.traffic_period}
                  {' · '}
                  {plan.speed_limit_mbps} Mbps
                  {plan.traffic_quota_bytes == null ? ' · 流量不限' : ''}
                </p>
                {plan.description ? <p className="mt-2 text-sm text-muted-foreground">{plan.description}</p> : null}
              </div>
              <Button loading={purchase.isPending} onClick={() => { setMessage(''); purchase.mutate(plan.id) }}>
                购买
              </Button>
            </article>
          ))}
          {!plans.query.isPending && plans.items.length === 0 ? (
            <p className="page-card p-5 text-sm text-muted-foreground">暂无可购买的套餐</p>
          ) : null}
          <ListPager page={plans.pagination.page} size={plans.pagination.size} total={plans.pagination.total} onPageChange={plans.setPage} onSizeChange={plans.setSize} />
        </div>
      )}
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
