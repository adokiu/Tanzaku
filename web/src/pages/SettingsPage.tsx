import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import apiClient from '@/api/client'
import { Button } from '@/components/Button'

type Setting = { key: string; value: unknown; updated_at: string }

const labels: Record<string, string> = {
  registration_enabled: '开放自助注册',
}

export default function SettingsPage() {
  const queryClient = useQueryClient()
  const query = useQuery({
    queryKey: ['admin-settings'],
    queryFn: async () => (await apiClient.get('/v1/admin/settings')).data as Setting[],
  })
  const update = useMutation({
    mutationFn: ({ key, value }: { key: string; value: boolean }) => apiClient.post('/v1/admin/settings', { key, value }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['admin-settings'] }),
  })

  return <section className="page-container">
    <header className="page-header"><h1 className="page-title">系统设置</h1></header>
    {query.isError ? <p className="page-card p-5 text-sm text-apple-red" role="alert">{apiError(query.error)}</p> : (
      <div className="grid gap-3">
        {(query.data ?? []).map((setting) => (
          <div className="page-card flex items-center justify-between gap-4 p-5" key={setting.key}>
            <div><p className="font-medium">{labels[setting.key] ?? setting.key}</p><p className="mt-1 text-xs text-muted-foreground">{setting.key} {setting.updated_at}</p></div>
            {typeof setting.value === 'boolean' ? <Button type="button" variant={setting.value ? 'primary' : 'secondary'} loading={update.isPending} onClick={() => update.mutate({ key: setting.key, value: !setting.value })}>{setting.value ? '已启用' : '已关闭'}</Button> : <code className="text-xs">{JSON.stringify(setting.value)}</code>}
          </div>
        ))}
      </div>
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
