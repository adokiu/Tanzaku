import { useEffect, useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { getPageItems } from '@/api/page'
import { Button } from '@/components/Button'
import { FormField } from '@/components/FormField'
import { Input } from '@/components/Input'
import { PageListHeader } from '@/components/PageListHeader'
import { Select } from '@/components/Select'
import { translateApiError } from '@/i18n/apiError'
import { useBrandingStore } from '@/stores/branding'
import { toast } from '@/stores/toast'
import './SettingsPage.css'

type Setting = { key: string; value: unknown; updated_at: string }
type PlanOption = { id: string; name: string; enabled: boolean }
type TabId = 'site' | 'security' | 'mail'

const CLIENT_IP_GEO_PROVIDERS = ['ipinfo', 'ip9', 'ip_sb'] as const
const TRAFFIC_RESET_MODES = ['month_first', 'month_purchase', 'never', 'year_first', 'year_purchase'] as const

const SITE_KEYS = [
  'site_title',
  'site_subtitle',
  'site_description',
  'site_url',
  'registration_enabled',
  'trial_plan_id',
  'trial_duration_days',
  'traffic_reset_mode',
  'client_ip_geo_provider',
  'node_host_metrics_interval_secs',
] as const

const SECURITY_KEYS = [
  'security_email_verification',
  'security_safe_mode',
  'security_email_suffix_whitelist_enabled',
  'security_email_suffix_whitelist',
  'security_captcha_enabled',
  'security_ip_register_limit_enabled',
  'security_ip_register_max_count',
  'security_ip_register_window_minutes',
  'security_password_attempt_limit_enabled',
  'security_password_attempt_max',
  'security_password_lock_minutes',
] as const

const MAIL_KEYS = [
  'mail_smtp_host',
  'mail_smtp_port',
  'mail_smtp_encryption',
  'mail_smtp_username',
  'mail_smtp_password',
  'mail_from_address',
  'mail_notify_enabled',
] as const

type FormState = Record<string, unknown>

function settingsMap(rows: Setting[] | undefined): FormState {
  const map: FormState = {}
  for (const row of rows ?? []) map[row.key] = row.value
  return map
}

function asString(value: unknown, fallback = ''): string {
  if (typeof value === 'string') return value
  if (value == null) return fallback
  return String(value)
}

function asBool(value: unknown, fallback = false): boolean {
  return typeof value === 'boolean' ? value : fallback
}

function asNumber(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

function whitelistToText(value: unknown): string {
  if (Array.isArray(value)) return value.map(String).join('\n')
  return ''
}

function textToWhitelist(text: string): string[] {
  return text
    .split(/[\n,]+/)
    .map((item) => item.trim().replace(/^@/, ''))
    .filter(Boolean)
}

function SwitchRow({
  label,
  hint,
  checked,
  onChange,
}: {
  label: string
  hint?: string
  checked: boolean
  onChange: (next: boolean) => void
}) {
  const { t } = useTranslation()
  return (
    <div className="settings-switch-row">
      <div className="settings-switch-row__text">
        <p className="settings-switch-row__label">{label}</p>
        {hint ? <p className="settings-switch-row__hint">{hint}</p> : null}
      </div>
      <Button type="button" size="sm" variant={checked ? 'primary' : 'secondary'} onClick={() => onChange(!checked)}>
        {checked ? t('common.enabled') : t('common.disabled')}
      </Button>
    </div>
  )
}

export default function SettingsPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [tab, setTab] = useState<TabId>('site')
  const [form, setForm] = useState<FormState>({})
  const [passwordDraft, setPasswordDraft] = useState('')
  const [testMailTo, setTestMailTo] = useState('')

  const query = useQuery({
    queryKey: ['admin-settings'],
    queryFn: async () => (await apiClient.get('/v1/admin/settings')).data as Setting[],
  })
  const plans = useQuery({
    queryKey: ['admin-plans-options'],
    queryFn: () => getPageItems<PlanOption>('/v1/admin/plans'),
  })

  useEffect(() => {
    if (!query.data) return
    const map = settingsMap(query.data)
    setForm(map)
    setPasswordDraft('')
  }, [query.data])

  const planSelectOptions = useMemo(
    () => [
      { label: t('settings.trialNone'), value: '' },
      ...(plans.data ?? [])
        .filter((plan) => plan.enabled)
        .map((plan) => ({ label: plan.name, value: plan.id })),
    ],
    [plans.data, t],
  )
  const geoSelectOptions = useMemo(
    () => CLIENT_IP_GEO_PROVIDERS.map((provider) => ({
      label: t(`settingsGeoProvider.${provider}`),
      value: provider,
    })),
    [t],
  )
  const trafficResetOptions = useMemo(
    () => TRAFFIC_RESET_MODES.map((mode) => ({
      label: t(`enums.billingPeriod.${mode}`),
      value: mode,
    })),
    [t],
  )
  const encryptionOptions = useMemo(
    () => [
      { label: t('settings.mailEncryption.none'), value: 'none' },
      { label: t('settings.mailEncryption.starttls'), value: 'starttls' },
      { label: t('settings.mailEncryption.ssl'), value: 'ssl' },
    ],
    [t],
  )

  const save = useMutation({
    mutationFn: async (keys: readonly string[]) => {
      for (const key of keys) {
        let value = form[key]
        if (key === 'security_email_suffix_whitelist') {
          value = textToWhitelist(asString(form.security_email_suffix_whitelist_text ?? whitelistToText(form[key])))
        }
        if (key === 'trial_plan_id') {
          const raw = asString(value).trim()
          value = raw ? raw : null
        }
        if (key === 'mail_smtp_password') {
          if (!passwordDraft.trim()) continue
          value = passwordDraft
        }
        if (key === 'node_host_metrics_interval_secs' || key === 'trial_duration_days'
          || key === 'mail_smtp_port' || key === 'security_ip_register_max_count'
          || key === 'security_ip_register_window_minutes' || key === 'security_password_attempt_max'
          || key === 'security_password_lock_minutes') {
          value = Number(value)
        }
        await apiClient.post('/v1/admin/settings', { key, value })
      }
    },
    onSuccess: async (_data, keys) => {
      setPasswordDraft('')
      await queryClient.invalidateQueries({ queryKey: ['admin-settings'] })
      if (keys.some((key) => key.startsWith('site_'))) {
        await useBrandingStore.getState().load(true)
      }
      toast.success(t('common.saveSuccess'))
    },
    onError: (err) => {
      toast.error(translateApiError(t, err))
    },
  })

  const testMail = useMutation({
    mutationFn: async () => {
      // 先保存邮件配置，再按库内配置发送
      await save.mutateAsync(MAIL_KEYS)
      await apiClient.post('/v1/admin/settings/mail/test', { to: testMailTo.trim() })
    },
    onSuccess: () => {
      toast.success(t('settings.mailTestSuccess'))
    },
    onError: (err) => {
      toast.error(translateApiError(t, err))
    },
  })

  function patch(key: string, value: unknown) {
    setForm((current) => ({ ...current, [key]: value }))
  }

  const whitelistText = asString(
    form.security_email_suffix_whitelist_text,
    whitelistToText(form.security_email_suffix_whitelist),
  )
  const passwordSet = Boolean(
    form.mail_smtp_password
    && typeof form.mail_smtp_password === 'object'
    && (form.mail_smtp_password as { set?: boolean }).set,
  )

  const tabs: { id: TabId; label: string }[] = [
    { id: 'site', label: t('settings.tabs.site') },
    { id: 'security', label: t('settings.tabs.security') },
    { id: 'mail', label: t('settings.tabs.mail') },
  ]

  return (
    <section className="page-container settings-page">
      <PageListHeader />
      <div className="settings-tabs" role="tablist">
        {tabs.map((item) => (
          <button
            key={item.id}
            type="button"
            role="tab"
            aria-selected={tab === item.id}
            className={`settings-tabs__btn${tab === item.id ? ' is-active' : ''}`}
            onClick={() => setTab(item.id)}
          >
            {item.label}
          </button>
        ))}
      </div>

      {query.isError ? (
        <p className="page-card p-5 text-sm text-apple-red settings-error" role="alert">
          {translateApiError(t, query.error)}
        </p>
      ) : null}

      {tab === 'site' ? (
        <>
          <div className="page-card settings-section">
            <h2 className="settings-section__title">{t('settings.sections.siteInfo')}</h2>
            <p className="settings-section__desc">{t('settings.sections.siteInfoDesc')}</p>
            <div className="settings-grid">
              <FormField label={t('settings.site_title')} hint={t('settings.hints.site_title')}>
                <Input value={asString(form.site_title)} onChange={(e) => patch('site_title', e.target.value)} />
              </FormField>
              <FormField label={t('settings.site_subtitle')} hint={t('settings.hints.site_subtitle')}>
                <Input value={asString(form.site_subtitle)} onChange={(e) => patch('site_subtitle', e.target.value)} />
              </FormField>
              <FormField label={t('settings.site_description')} hint={t('settings.hints.site_description')}>
                <textarea
                  className="settings-textarea"
                  value={asString(form.site_description)}
                  onChange={(e) => patch('site_description', e.target.value)}
                />
              </FormField>
              <FormField label={t('settings.site_url')} hint={t('settings.hints.site_url')}>
                <Input
                  placeholder="https://example.com"
                  value={asString(form.site_url)}
                  onChange={(e) => patch('site_url', e.target.value)}
                />
              </FormField>
            </div>
          </div>

          <div className="page-card settings-section">
            <h2 className="settings-section__title">{t('settings.sections.trial')}</h2>
            <p className="settings-section__desc">{t('settings.sections.trialDesc')}</p>
            <div className="settings-grid settings-grid--2">
              <FormField label={t('settings.trial_plan_id')} hint={t('settings.hints.trial_plan_id')}>
                <Select
                  value={asString(form.trial_plan_id)}
                  options={planSelectOptions}
                  onChange={(value) => patch('trial_plan_id', String(value))}
                />
              </FormField>
              <FormField label={t('settings.trial_duration_days')} hint={t('settings.hints.trial_duration_days')}>
                <Input
                  type="number"
                  min={0}
                  max={3650}
                  value={String(asNumber(form.trial_duration_days, 7))}
                  onChange={(e) => patch('trial_duration_days', Number(e.target.value))}
                />
              </FormField>
            </div>
            <SwitchRow
              label={t('settings.registration_enabled')}
              hint={t('settings.hints.registration_enabled')}
              checked={asBool(form.registration_enabled)}
              onChange={(next) => patch('registration_enabled', next)}
            />
          </div>

          <div className="page-card settings-section">
            <h2 className="settings-section__title">{t('settings.sections.ops')}</h2>
            <p className="settings-section__desc">{t('settings.sections.opsDesc')}</p>
            <div className="settings-grid settings-grid--2">
              <FormField label={t('settings.traffic_reset_mode')} hint={t('settings.hints.traffic_reset_mode')}>
                <Select
                  value={asString(form.traffic_reset_mode, 'month_purchase')}
                  options={trafficResetOptions}
                  onChange={(value) => patch('traffic_reset_mode', String(value))}
                />
              </FormField>
              <FormField label={t('settings.client_ip_geo_provider')}>
                <Select
                  value={asString(form.client_ip_geo_provider, 'ipinfo')}
                  options={geoSelectOptions}
                  onChange={(value) => patch('client_ip_geo_provider', String(value))}
                />
              </FormField>
              <FormField label={`${t('settings.node_host_metrics_interval_secs')}（${t('settings.secondsUnit')}）`}>
                <Input
                  type="number"
                  min={1}
                  max={3600}
                  value={String(asNumber(form.node_host_metrics_interval_secs, 1))}
                  onChange={(e) => patch('node_host_metrics_interval_secs', Number(e.target.value))}
                />
              </FormField>
            </div>
          </div>

          <div className="settings-footer">
            <Button loading={save.isPending} onClick={() => save.mutate(SITE_KEYS)}>{t('common.save')}</Button>
          </div>
        </>
      ) : null}

      {tab === 'security' ? (
        <>
          <div className="page-card settings-section">
            <h2 className="settings-section__title">{t('settings.sections.siteSecurity')}</h2>
            <p className="settings-section__desc">{t('settings.sections.siteSecurityDesc')}</p>
            <SwitchRow
              label={t('settings.security_email_verification')}
              hint={t('settings.hints.security_email_verification')}
              checked={asBool(form.security_email_verification)}
              onChange={(next) => patch('security_email_verification', next)}
            />
            <SwitchRow
              label={t('settings.security_safe_mode')}
              hint={t('settings.hints.security_safe_mode')}
              checked={asBool(form.security_safe_mode)}
              onChange={(next) => patch('security_safe_mode', next)}
            />
            <SwitchRow
              label={t('settings.security_email_suffix_whitelist_enabled')}
              hint={t('settings.hints.security_email_suffix_whitelist_enabled')}
              checked={asBool(form.security_email_suffix_whitelist_enabled)}
              onChange={(next) => patch('security_email_suffix_whitelist_enabled', next)}
            />
            {asBool(form.security_email_suffix_whitelist_enabled) ? (
              <FormField label={t('settings.security_email_suffix_whitelist')} hint={t('settings.hints.security_email_suffix_whitelist')}>
                <textarea
                  className="settings-textarea"
                  value={whitelistText}
                  onChange={(e) => patch('security_email_suffix_whitelist_text', e.target.value)}
                  placeholder={'gmail.com\nexample.com'}
                />
              </FormField>
            ) : null}
            <SwitchRow
              label={t('settings.security_captcha_enabled')}
              hint={t('settings.hints.security_captcha_enabled')}
              checked={asBool(form.security_captcha_enabled)}
              onChange={(next) => patch('security_captcha_enabled', next)}
            />
          </div>

          <div className="page-card settings-section">
            <h2 className="settings-section__title">{t('settings.sections.registerLimit')}</h2>
            <SwitchRow
              label={t('settings.security_ip_register_limit_enabled')}
              hint={t('settings.hints.security_ip_register_limit_enabled')}
              checked={asBool(form.security_ip_register_limit_enabled)}
              onChange={(next) => patch('security_ip_register_limit_enabled', next)}
            />
            {asBool(form.security_ip_register_limit_enabled) ? (
              <div className="settings-grid settings-grid--2">
                <FormField label={t('settings.security_ip_register_max_count')} hint={t('settings.hints.security_ip_register_max_count')}>
                  <Input
                    type="number"
                    min={1}
                    value={String(asNumber(form.security_ip_register_max_count, 3))}
                    onChange={(e) => patch('security_ip_register_max_count', Number(e.target.value))}
                  />
                </FormField>
                <FormField label={t('settings.security_ip_register_window_minutes')} hint={t('settings.hints.security_ip_register_window_minutes')}>
                  <Input
                    type="number"
                    min={1}
                    value={String(asNumber(form.security_ip_register_window_minutes, 60))}
                    onChange={(e) => patch('security_ip_register_window_minutes', Number(e.target.value))}
                  />
                </FormField>
              </div>
            ) : null}
          </div>

          <div className="page-card settings-section">
            <h2 className="settings-section__title">{t('settings.sections.passwordLimit')}</h2>
            <SwitchRow
              label={t('settings.security_password_attempt_limit_enabled')}
              hint={t('settings.hints.security_password_attempt_limit_enabled')}
              checked={asBool(form.security_password_attempt_limit_enabled)}
              onChange={(next) => patch('security_password_attempt_limit_enabled', next)}
            />
            {asBool(form.security_password_attempt_limit_enabled) ? (
              <div className="settings-grid settings-grid--2">
                <FormField label={t('settings.security_password_attempt_max')} hint={t('settings.hints.security_password_attempt_max')}>
                  <Input
                    type="number"
                    min={1}
                    value={String(asNumber(form.security_password_attempt_max, 5))}
                    onChange={(e) => patch('security_password_attempt_max', Number(e.target.value))}
                  />
                </FormField>
                <FormField label={t('settings.security_password_lock_minutes')} hint={t('settings.hints.security_password_lock_minutes')}>
                  <Input
                    type="number"
                    min={0}
                    value={String(asNumber(form.security_password_lock_minutes, 0))}
                    onChange={(e) => patch('security_password_lock_minutes', Number(e.target.value))}
                  />
                </FormField>
              </div>
            ) : null}
          </div>

          <div className="settings-footer">
            <Button loading={save.isPending} onClick={() => save.mutate(SECURITY_KEYS)}>{t('common.save')}</Button>
          </div>
        </>
      ) : null}

      {tab === 'mail' ? (
        <>
          <div className="page-card settings-section">
            <h2 className="settings-section__title">{t('settings.sections.mail')}</h2>
            <p className="settings-section__desc">{t('settings.sections.mailDesc')}</p>
            <div className="settings-grid settings-grid--2">
              <FormField label={t('settings.mail_smtp_host')} hint={t('settings.hints.mail_smtp_host')}>
                <Input value={asString(form.mail_smtp_host)} onChange={(e) => patch('mail_smtp_host', e.target.value)} />
              </FormField>
              <FormField label={t('settings.mail_smtp_port')} hint={t('settings.hints.mail_smtp_port')}>
                <Input
                  type="number"
                  min={1}
                  max={65535}
                  value={String(asNumber(form.mail_smtp_port, 465))}
                  onChange={(e) => patch('mail_smtp_port', Number(e.target.value))}
                />
              </FormField>
              <FormField label={t('settings.mail_smtp_encryption')} hint={t('settings.hints.mail_smtp_encryption')}>
                <Select
                  value={asString(form.mail_smtp_encryption, 'ssl')}
                  options={encryptionOptions}
                  onChange={(value) => patch('mail_smtp_encryption', String(value))}
                />
              </FormField>
              <FormField label={t('settings.mail_from_address')} hint={t('settings.hints.mail_from_address')}>
                <Input value={asString(form.mail_from_address)} onChange={(e) => patch('mail_from_address', e.target.value)} />
              </FormField>
              <FormField label={t('settings.mail_smtp_username')} hint={t('settings.hints.mail_smtp_username')}>
                <Input value={asString(form.mail_smtp_username)} onChange={(e) => patch('mail_smtp_username', e.target.value)} />
              </FormField>
              <FormField
                label={t('settings.mail_smtp_password')}
                hint={passwordSet ? t('settings.hints.mail_smtp_password_set') : t('settings.hints.mail_smtp_password')}
              >
                <Input
                  type="password"
                  autoComplete="new-password"
                  placeholder={passwordSet ? '••••••••' : ''}
                  value={passwordDraft}
                  onChange={(e) => setPasswordDraft(e.target.value)}
                />
              </FormField>
            </div>
            <SwitchRow
              label={t('settings.mail_notify_enabled')}
              hint={t('settings.hints.mail_notify_enabled')}
              checked={asBool(form.mail_notify_enabled)}
              onChange={(next) => patch('mail_notify_enabled', next)}
            />
            <div className="settings-mail-test">
              <div className="settings-mail-test__field">
                <FormField label={t('settings.mailTestTo')} hint={t('settings.hints.mailTestTo')}>
                  <Input
                    type="email"
                    value={testMailTo}
                    onChange={(e) => setTestMailTo(e.target.value)}
                    placeholder="you@example.com"
                  />
                </FormField>
              </div>
              <Button
                variant="secondary"
                loading={testMail.isPending}
                disabled={!testMailTo.trim()}
                onClick={() => testMail.mutate()}
              >
                {t('settings.mailTestSend')}
              </Button>
            </div>
          </div>
          <div className="settings-footer">
            <Button loading={save.isPending} onClick={() => save.mutate(MAIL_KEYS)}>{t('common.save')}</Button>
          </div>
        </>
      ) : null}
    </section>
  )
}
