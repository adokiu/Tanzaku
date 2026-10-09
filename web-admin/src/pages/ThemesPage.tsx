import { useEffect, useMemo, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { RefreshCw, Upload } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { Button } from '@/components/Button'
import { type Column } from '@/components/DataTable'
import { EntityList } from '@/components/EntityListPage/EntityListPage'
import { FormField, FormStack } from '@/components/FormField'
import { Input } from '@/components/Input'
import { PageListHeader } from '@/components/PageListHeader'
import { Select } from '@/components/Select'
import { translateApiError } from '@/i18n/apiError'
import { toast } from '@/stores/toast'
import './ThemesPage.css'

type ThemeRow = {
  name: string
  short: string
  description?: string | null
  version: string
  author: string
  url?: string | null
  preview?: string | null
  active: boolean
  path: string
}

type ManagedField = {
  key?: string
  name?: string | Record<string, string>
  help?: string | Record<string, string>
  type: string
  default?: unknown
  options?: string
  required?: boolean
}

type ThemeSettingsResponse = {
  short: string
  configuration?: {
    type?: string
    name?: string | Record<string, string>
    data?: ManagedField[]
  } | null
  settings: Record<string, unknown>
}

function displayText(value: unknown): string {
  if (typeof value === 'string') return value
  if (value && typeof value === 'object') {
    const map = value as Record<string, string>
    return map['zh-CN'] || map.zh || map.en || Object.values(map)[0] || ''
  }
  return ''
}

export default function ThemesPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const fileRef = useRef<HTMLInputElement>(null)
  const [uploading, setUploading] = useState(false)
  const [editingShort, setEditingShort] = useState<string | null>(null)
  const [draft, setDraft] = useState<Record<string, unknown>>({})

  const activeQuery = useQuery({
    queryKey: ['admin-themes-hint'],
    queryFn: async () => {
      const response = await apiClient.get('/v1/admin/themes', { params: { page: 1, page_size: 1 } })
      return response.data as { total?: number }
    },
  })

  const settingsQuery = useQuery({
    queryKey: ['admin-theme-settings', editingShort],
    enabled: Boolean(editingShort),
    queryFn: async () => {
      const response = await apiClient.get(`/v1/admin/themes/${encodeURIComponent(editingShort!)}/settings`)
      return response.data as ThemeSettingsResponse
    },
  })

  useEffect(() => {
    if (!settingsQuery.data) return
    setDraft({ ...settingsQuery.data.settings })
  }, [settingsQuery.data])

  const fields = useMemo(() => {
    const data = settingsQuery.data?.configuration?.data
    return Array.isArray(data) ? data : []
  }, [settingsQuery.data])

  const invalidate = async () => {
    await queryClient.invalidateQueries({ queryKey: ['admin-themes'] })
    await queryClient.invalidateQueries({ queryKey: ['admin-themes-hint'] })
    if (editingShort) {
      await queryClient.invalidateQueries({ queryKey: ['admin-theme-settings', editingShort] })
    }
  }

  const setActive = useMutation({
    mutationFn: async (short: string) => {
      await apiClient.post(`/v1/admin/themes/active?theme=${encodeURIComponent(short)}`)
    },
    onSuccess: async () => {
      toast.success(t('themes.activated'))
      await invalidate()
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const remove = useMutation({
    mutationFn: async (short: string) => {
      await apiClient.delete(`/v1/admin/themes/${encodeURIComponent(short)}`)
    },
    onSuccess: async (_data, short) => {
      toast.success(t('common.deleteSuccess'))
      if (editingShort === short) setEditingShort(null)
      await invalidate()
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  const saveSettings = useMutation({
    mutationFn: async () => {
      if (!editingShort) return
      await apiClient.post(`/v1/admin/themes/${encodeURIComponent(editingShort)}/settings`, draft)
    },
    onSuccess: async () => {
      toast.success(t('themes.settingsSaved'))
      await invalidate()
    },
    onError: (error) => toast.error(translateApiError(t, error)),
  })

  async function onPickFile(file: File | null) {
    if (!file) return
    if (!file.name.toLowerCase().endsWith('.zip')) {
      toast.error(t('themes.zipOnly'))
      return
    }
    setUploading(true)
    try {
      const form = new FormData()
      form.append('file', file)
      await apiClient.post('/v1/admin/themes/upload', form, {
        timeout: 120_000,
        headers: { 'Content-Type': 'multipart/form-data' },
        transformRequest: [(data, headers) => {
          if (headers && typeof (headers as { delete?: (k: string) => void }).delete === 'function') {
            ;(headers as { delete: (k: string) => void }).delete('Content-Type')
          }
          return data
        }],
      })
      toast.success(t('themes.uploadSuccess'))
      await invalidate()
    } catch (error) {
      toast.error(translateApiError(t, error))
    } finally {
      setUploading(false)
      if (fileRef.current) fileRef.current.value = ''
    }
  }

  const columns: Column<ThemeRow>[] = useMemo(
    () => [
      {
        key: 'name',
        title: t('themes.name'),
        render: (row) => (
          <div className="min-w-0">
            <div className="data-table-cell font-medium">{row.name}</div>
            {row.description ? (
              <div className="mt-0.5 truncate text-xs text-muted-foreground">{row.description}</div>
            ) : null}
          </div>
        ),
      },
      {
        key: 'short',
        title: t('themes.short'),
        width: 120,
        render: (row) => <span className="data-table-cell font-mono text-xs">{row.short}</span>,
      },
      {
        key: 'version',
        title: t('themes.version'),
        width: 88,
        render: (row) => <span className="data-table-cell">{row.version}</span>,
      },
      {
        key: 'author',
        title: t('themes.author'),
        width: 120,
        render: (row) => <span className="data-table-cell">{row.author || '—'}</span>,
      },
      {
        key: 'path',
        title: t('themes.path'),
        width: 180,
        render: (row) => (
          <span className="data-table-cell font-mono text-xs" title={`$TANZAKU_DATA/${row.path}`}>
            {row.path}
          </span>
        ),
      },
      {
        key: 'active',
        title: t('themes.status'),
        width: 88,
        fixedWidth: true,
        render: (row) => (
          <span
            className={`data-table-tag entity-list-cell__status-tag ${
              row.active ? 'data-table-tag--online' : 'data-table-tag--neutral'
            }`}
          >
            {row.active ? t('themes.active') : t('themes.inactive')}
          </span>
        ),
      },
      {
        key: 'actions',
        title: t('fields.action'),
        render: (row) => (
          <div className="data-table-actions">
            <button
              type="button"
              className="data-table-link-btn"
              onClick={() => setEditingShort(row.short)}
            >
              {t('themes.configure')}
            </button>
            <button
              type="button"
              className="data-table-link-btn"
              disabled={row.active || setActive.isPending}
              onClick={() => setActive.mutate(row.short)}
            >
              {t('themes.activate')}
            </button>
            <button
              type="button"
              className="data-table-link-btn data-table-link-btn--danger"
              disabled={remove.isPending}
              onClick={() => {
                if (window.confirm(t('themes.deleteConfirm', { name: row.name }))) {
                  remove.mutate(row.short)
                }
              }}
            >
              {t('common.delete')}
            </button>
          </div>
        ),
      },
    ],
    [remove, setActive, t],
  )

  return (
    <section className="page-container page-container--entity-list">
      <PageListHeader
        actions={(
          <>
            <input
              ref={fileRef}
              type="file"
              accept=".zip,application/zip"
              className="hidden"
              onChange={(event) => void onPickFile(event.target.files?.[0] ?? null)}
            />
            <Button size="sm" loading={uploading} onClick={() => fileRef.current?.click()}>
              <Upload size={16} />
              {t('themes.upload')}
            </Button>
            <Button variant="secondary" size="sm" onClick={() => void invalidate()}>
              <RefreshCw size={16} />
              {t('common.refresh')}
            </Button>
          </>
        )}
      />
      <p className="mb-3 text-sm text-muted-foreground">
        {t('themes.hint', { total: activeQuery.data?.total ?? 0 })}
      </p>
      <EntityList
        columns={columns}
        queryKey={['admin-themes']}
        fetchPage={async (params) => {
          const response = await apiClient.get('/v1/admin/themes', { params })
          return response.data as { items: ThemeRow[]; total: number; page: number; page_size: number }
        }}
        rowKey={(row) => row.short}
      />

      {editingShort ? (
        <div className="theme-settings-panel">
          <div className="theme-settings-panel__header">
            <div>
              <h2 className="theme-settings-panel__title">{t('themes.settingsTitle')}</h2>
              <p className="theme-settings-panel__meta">{editingShort}</p>
            </div>
            <div className="theme-settings-panel__actions">
              <Button variant="secondary" size="sm" onClick={() => setEditingShort(null)}>
                {t('common.cancel')}
              </Button>
              <Button
                size="sm"
                loading={saveSettings.isPending}
                onClick={() => saveSettings.mutate()}
                disabled={settingsQuery.isLoading || fields.length === 0}
              >
                {t('common.save')}
              </Button>
            </div>
          </div>
          {settingsQuery.isLoading ? (
            <p className="text-sm text-muted-foreground">{t('common.loading')}</p>
          ) : fields.length === 0 ? (
            <p className="text-sm text-muted-foreground">{t('themes.noManagedSettings')}</p>
          ) : (
            <FormStack>
              {fields.map((field, index) => {
                if (field.type === 'title') {
                  return (
                    <h3 key={`title-${index}`} className="theme-settings-panel__section">
                      {displayText(field.name)}
                    </h3>
                  )
                }
                if (field.type === 'textbox') {
                  return (
                    <p key={`textbox-${index}`} className="theme-settings-panel__help">
                      {displayText(field.name)}
                    </p>
                  )
                }
                if (!field.key) return null
                const key = field.key
                const label = displayText(field.name) || key
                const hint = displayText(field.help)
                if (field.type === 'switch') {
                  const checked = Boolean(draft[key])
                  return (
                    <div key={key} className="theme-settings-switch">
                      <div>
                        <p className="theme-settings-switch__label">{label}</p>
                        {hint ? <p className="theme-settings-switch__hint">{hint}</p> : null}
                      </div>
                      <Button
                        type="button"
                        size="sm"
                        variant={checked ? 'primary' : 'secondary'}
                        onClick={() => setDraft((prev) => ({ ...prev, [key]: !checked }))}
                      >
                        {checked ? t('common.enabled') : t('common.disabled')}
                      </Button>
                    </div>
                  )
                }
                if (field.type === 'select') {
                  const options = (field.options || '')
                    .split(',')
                    .map((item) => item.trim())
                    .filter(Boolean)
                    .map((item) => ({ value: item, label: item }))
                  return (
                    <FormField key={key} label={label} hint={hint || undefined} required={field.required}>
                      <Select
                        value={String(draft[key] ?? field.default ?? '')}
                        options={options}
                        onChange={(value) => setDraft((prev) => ({ ...prev, [key]: value }))}
                      />
                    </FormField>
                  )
                }
                if (field.type === 'number') {
                  return (
                    <FormField key={key} label={label} hint={hint || undefined} required={field.required}>
                      <Input
                        type="number"
                        value={String(draft[key] ?? field.default ?? 0)}
                        onChange={(event) => {
                          const raw = event.target.value
                          const next = raw === '' ? 0 : Number(raw)
                          setDraft((prev) => ({ ...prev, [key]: Number.isFinite(next) ? next : 0 }))
                        }}
                      />
                    </FormField>
                  )
                }
                if (field.type === 'richtext') {
                  return (
                    <FormField key={key} label={label} hint={hint || undefined} required={field.required}>
                      <textarea
                        className="input-field min-h-28 w-full text-sm"
                        value={String(draft[key] ?? '')}
                        onChange={(event) => setDraft((prev) => ({ ...prev, [key]: event.target.value }))}
                      />
                    </FormField>
                  )
                }
                return (
                  <FormField key={key} label={label} hint={hint || undefined} required={field.required}>
                    <Input
                      value={String(draft[key] ?? '')}
                      onChange={(event) => setDraft((prev) => ({ ...prev, [key]: event.target.value }))}
                    />
                  </FormField>
                )
              })}
            </FormStack>
          )}
        </div>
      ) : null}
    </section>
  )
}
