import { useCallback, useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { Button } from '@/components/Button'
import { type Column } from '@/components/DataTable'
import { EntityList } from '@/components/EntityListPage/EntityListPage'
import { FormField, FormStack } from '@/components/FormField'
import { GenericSidebar } from '@/components/GenericSidebar/GenericSidebar'
import { Input } from '@/components/Input'
import { PageListHeader } from '@/components/PageListHeader'
import { Select } from '@/components/Select/Select'
import { translateApiError } from '@/i18n/apiError'
import { toast } from '@/stores/toast'

type ConfigField = { key: string; label: string; secret: boolean; required: boolean }
type Driver = { kind: string; display_name: string; config_fields: ConfigField[] }
type Channel = {
  id: string
  name: string
  kind: string
  enabled: boolean
  sort_order: number
  config: Record<string, string>
}

const emptyForm = {
  name: '',
  kind: 'balance',
  enabled: true,
  sort_order: '0',
  config: {} as Record<string, string>,
}

export default function PaymentChannelsPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [editingId, setEditingId] = useState<string | null>(null)
  const [form, setForm] = useState(emptyForm)

  const drivers = useQuery({
    queryKey: ['admin-payment-drivers'],
    queryFn: async () => (await apiClient.get('/v1/admin/payment-drivers')).data as Driver[],
  })
  const driver = (drivers.data ?? []).find((item) => item.kind === form.kind)

  const invalidate = async () => {
    await queryClient.invalidateQueries({ queryKey: ['admin-payment-channels'] })
  }

  const save = useMutation({
    mutationFn: async () => {
      const payload = {
        name: form.name.trim(),
        kind: form.kind,
        enabled: form.enabled,
        sort_order: Number(form.sort_order) || 0,
        config: form.config,
      }
      if (editingId) {
        await apiClient.put(`/v1/admin/payment-channels/${editingId}`, payload)
      } else {
        await apiClient.post('/v1/admin/payment-channels', payload)
      }
    },
    onSuccess: async () => {
      setSidebarOpen(false)
      setEditingId(null)
      setForm(emptyForm)
      await invalidate()
      toast.success(t('common.saveSuccess'))
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const remove = useMutation({
    mutationFn: async (id: string) => {
      await apiClient.delete(`/v1/admin/payment-channels/${id}`)
    },
    onSuccess: async () => {
      await invalidate()
      toast.success(t('common.deleteSuccess'))
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const openCreate = useCallback(() => {
    setEditingId(null)
    setForm({
      ...emptyForm,
      kind: drivers.data?.[0]?.kind ?? 'balance',
    })
    setSidebarOpen(true)
  }, [drivers.data])

  const openEdit = useCallback((row: Channel) => {
    setEditingId(row.id)
    setForm({
      name: row.name,
      kind: row.kind,
      enabled: row.enabled,
      sort_order: String(row.sort_order ?? 0),
      config: { ...(row.config ?? {}) },
    })
    setSidebarOpen(true)
  }, [])

  const columns: Column<Channel>[] = useMemo(
    () => [
      { key: 'name', title: t('payments.name'), render: (row) => row.name },
      {
        key: 'kind',
        title: t('payments.kind'),
        render: (row) => (drivers.data ?? []).find((item) => item.kind === row.kind)?.display_name ?? row.kind,
      },
      {
        key: 'enabled',
        title: t('common.enabled'),
        render: (row) => (row.enabled ? t('common.enabled') : t('common.disabled')),
      },
      {
        key: 'actions',
        title: t('common.actions'),
        render: (row) => (
          <div className="data-table-actions">
            <button type="button" className="data-table-link-btn" onClick={() => openEdit(row)}>
              {t('common.edit')}
            </button>
            <button
              type="button"
              className="data-table-link-btn data-table-link-btn--danger"
              onClick={() => {
                if (window.confirm(t('payments.deleteConfirm', { name: row.name }))) {
                  remove.mutate(row.id)
                }
              }}
            >
              {t('common.delete')}
            </button>
          </div>
        ),
      },
    ],
    [drivers.data, openEdit, remove, t],
  )

  const fetchPage = useCallback(async () => {
    const items = (await apiClient.get('/v1/admin/payment-channels')).data as Channel[]
    return { items, total: items.length, page: 1, page_size: Math.max(items.length, 1) }
  }, [])

  return (
    <section className="page-container page-container--entity-list">
      <PageListHeader
        actions={(
          <>
            <Button size="sm" onClick={openCreate}>
              <Plus size={16} />
              {t('payments.create')}
            </Button>
            <Button variant="secondary" size="sm" onClick={() => void invalidate()}>
              <RefreshCw size={16} />
              {t('common.refresh')}
            </Button>
          </>
        )}
      />
      <EntityList
        columns={columns}
        queryKey={['admin-payment-channels']}
        fetchPage={fetchPage}
        rowKey={(row) => row.id}
      />
      <GenericSidebar
        open={sidebarOpen}
        title={editingId ? t('payments.edit') : t('payments.create')}
        onClose={() => setSidebarOpen(false)}
        footer={(
          <>
            <Button variant="ghost" onClick={() => setSidebarOpen(false)}>{t('common.cancel')}</Button>
            <Button loading={save.isPending} onClick={() => save.mutate()}>{t('common.save')}</Button>
          </>
        )}
      >
        <FormStack>
          <FormField label={t('payments.name')} required>
            <Input value={form.name} onChange={(event) => setForm((current) => ({ ...current, name: event.target.value }))} />
          </FormField>
          <FormField label={t('payments.kind')} required>
            <Select
              value={form.kind}
              options={(drivers.data ?? []).map((item) => ({ label: item.display_name, value: item.kind }))}
              onChange={(value) => setForm((current) => ({ ...current, kind: String(value), config: {} }))}
            />
          </FormField>
          {(driver?.config_fields ?? []).map((field) => (
            <FormField key={field.key} label={field.label} required={field.required}>
              <Input
                type={field.secret ? 'password' : 'text'}
                value={form.config[field.key] ?? ''}
                onChange={(event) => setForm((current) => ({
                  ...current,
                  config: { ...current.config, [field.key]: event.target.value },
                }))}
              />
            </FormField>
          ))}
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={form.enabled}
              onChange={(event) => setForm((current) => ({ ...current, enabled: event.target.checked }))}
            />
            {t('common.enabled')}
          </label>
        </FormStack>
      </GenericSidebar>
    </section>
  )
}
