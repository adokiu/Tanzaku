import { useState } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { Button } from '@/components/Button'
import { PageListHeader } from '@/components/PageListHeader'
import { translateApiError } from '@/i18n/apiError'
import { toast } from '@/stores/toast'
import { formatBalanceCents } from '@/utils/formatMoney'

type Order = {
  id: string
  order_no: string
  plan_name: string
  period: string
  amount_cents: number
  status: string
}

type Channel = {
  id: string
  name: string
  kind: string
}

export default function OrderPayPage() {
  const { t } = useTranslation()
  const { id } = useParams<{ id: string }>()
  const navigate = useNavigate()
  const [channelId, setChannelId] = useState<string>('')

  const orderQuery = useQuery({
    queryKey: ['user-order', id],
    enabled: Boolean(id),
    queryFn: async () => (await apiClient.get(`/v1/orders/${id}`)).data as Order,
  })
  const channelsQuery = useQuery({
    queryKey: ['user-payment-channels'],
    queryFn: async () => (await apiClient.get('/v1/payment-channels')).data as Channel[],
  })

  const pay = useMutation({
    mutationFn: async (payChannelId: string) => {
      const response = await apiClient.post(`/v1/orders/${id}/pay`, { channel_id: payChannelId })
      return response.data as { order: Order; pay_url?: string | null }
    },
    onSuccess: (result) => {
      if (result.pay_url) {
        window.location.assign(result.pay_url)
        return
      }
      toast.success(t('orders.payOk'))
      navigate('/orders', { replace: true })
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const order = orderQuery.data
  const channels = channelsQuery.data ?? []
  const selected = channelId || channels[0]?.id || ''

  return (
    <section className="page-container">
      <PageListHeader title={t('orders.payTitle')} />
      {orderQuery.isError ? (
        <p className="page-card p-5 text-sm text-destructive">{translateApiError(t, orderQuery.error)}</p>
      ) : !order ? (
        <p className="page-card p-5 text-sm text-muted-foreground">{t('common.loading')}</p>
      ) : (
        <div className="page-card flex flex-col gap-5 p-5">
          <div>
            <div className="text-base font-semibold">{order.plan_name}</div>
            <p className="mt-1 text-sm text-muted-foreground font-mono">{order.order_no}</p>
            <p className="mt-1 text-sm text-muted-foreground">
              {t(`plans.period.${order.period}`, { defaultValue: order.period })}
              {' · '}
              {formatBalanceCents(order.amount_cents)}
            </p>
            <p className="mt-1 text-sm text-muted-foreground">{t(`orders.statusValue.${order.status}`, { defaultValue: order.status })}</p>
          </div>
          {order.status === 'pending' ? (
            <>
              <div className="flex flex-col gap-2">
                <div className="text-sm font-medium">{t('orders.chooseChannel')}</div>
                {channels.length === 0 ? (
                  <p className="text-sm text-muted-foreground">{t('orders.noChannel')}</p>
                ) : (
                  channels.map((channel) => (
                    <label key={channel.id} className="flex items-center gap-2 text-sm">
                      <input
                        type="radio"
                        name="channel"
                        checked={selected === channel.id}
                        onChange={() => setChannelId(channel.id)}
                      />
                      {channel.name}
                    </label>
                  ))
                )}
              </div>
              <Button
                disabled={!selected || channels.length === 0}
                loading={pay.isPending}
                onClick={() => pay.mutate(selected)}
              >
                {t('orders.payNow')}
              </Button>
            </>
          ) : (
            <Link className="text-sm text-primary hover:underline" to="/orders">
              {t('orders.backToList')}
            </Link>
          )}
        </div>
      )}
    </section>
  )
}
