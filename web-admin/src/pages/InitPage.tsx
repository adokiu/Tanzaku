import { useEffect, useState, type FormEvent, type ReactNode } from 'react'
import { useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { Check, Database, Server } from 'lucide-react'
import apiClient from '@/api/client'
import { Button } from '@/components/Button'
import { translateApiError } from '@/i18n/apiError'
import { validatePassword } from '@/utils/passwordPolicy'

type InitFields = {
  pg_host: string
  pg_port: string
  pg_database: string
  pg_user: string
  pg_password: string
  pg_sslmode: string
  redis_host: string
  redis_port: string
  redis_database: string
  redis_password: string
  redis_tls: boolean
  admin_email: string
  admin_password: string
  admin_password_confirm: string
}

const initialFields: InitFields = {
  pg_host: '', pg_port: '5432', pg_database: '', pg_user: '', pg_password: '', pg_sslmode: 'prefer',
  redis_host: '', redis_port: '6379', redis_database: '0', redis_password: '', redis_tls: false,
  admin_email: '', admin_password: '', admin_password_confirm: '',
}

const sslModes = ['prefer', 'require', 'verify-ca', 'verify-full', 'disable'] as const

export default function InitPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [fields, setFields] = useState(initialFields)
  const [checking, setChecking] = useState(true)
  const [busy, setBusy] = useState<'postgres' | 'redis' | 'submit' | null>(null)
  const [message, setMessage] = useState('')
  const [finished, setFinished] = useState(false)

  useEffect(() => {
    apiClient.get('/init/status')
      .then((response) => {
        if (response.data.initialized) navigate('/login', { replace: true })
      })
      .catch(() => setMessage(t('init.cannotReachBoard')))
      .finally(() => setChecking(false))
  }, [navigate, t])

  function update<K extends keyof InitFields>(key: K, value: InitFields[K]) {
    setFields((current) => ({ ...current, [key]: value }))
    setMessage('')
  }

  function postgresPayload() {
    return {
      host: fields.pg_host,
      port: Number(fields.pg_port),
      database: fields.pg_database,
      user: fields.pg_user,
      password: fields.pg_password,
      sslmode: fields.pg_sslmode,
      max_connections: 32,
    }
  }

  function redisPayload() {
    return {
      host: fields.redis_host,
      port: Number(fields.redis_port),
      database: Number(fields.redis_database),
      password: fields.redis_password || null,
      tls: fields.redis_tls,
    }
  }

  async function testConnection(kind: 'postgres' | 'redis') {
    setBusy(kind)
    setMessage(t('init.testingConnection'))
    try {
      await apiClient.post(`/init/test/${kind}`, { [kind]: kind === 'postgres' ? postgresPayload() : redisPayload() })
      setMessage(kind === 'postgres' ? t('init.postgresOk') : t('init.redisOk'))
    } catch (cause) {
      setMessage(translateApiError(t, cause))
    } finally {
      setBusy(null)
    }
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (fields.admin_password !== fields.admin_password_confirm) {
      setMessage(t('init.passwordMismatch'))
      return
    }
    const passwordError = validatePassword(fields.admin_password)
    if (passwordError) {
      setMessage(t(passwordError))
      return
    }
    setBusy('submit')
    setMessage(t('init.initializing'))
    try {
      await apiClient.post('/init/complete', {
        postgres: postgresPayload(),
        redis: redisPayload(),
        admin_email: fields.admin_email,
        admin_password: fields.admin_password,
      })
      setFields(initialFields)
      setFinished(true)
      window.setTimeout(() => window.location.assign('/login'), 1200)
    } catch (cause) {
      setMessage(translateApiError(t, cause))
    } finally {
      setBusy(null)
    }
  }

  if (checking) {
    return <main className="min-h-screen flex items-center justify-center bg-background"><p className="text-sm text-muted-foreground">{t('errors.checkingInit')}</p></main>
  }

  return (
    <main className="min-h-screen bg-background flex items-center justify-center p-6">
      <div className="w-full max-w-2xl">
        <header className="mb-8 text-center">
          <h1 className="mb-2 text-3xl font-display font-semibold tracking-tight">{t('init.title')}</h1>
          <p className="text-sm text-muted-foreground">{t('init.subtitle')}</p>
        </header>
        {finished ? (
          <section className="glass-card p-8 text-center">
            <Check className="mx-auto mb-3 text-apple-green" size={32} />
            <h2 className="text-lg font-semibold">{t('init.doneTitle')}</h2>
            <p className="mt-2 text-sm text-muted-foreground">{t('init.doneSubtitle')}</p>
          </section>
        ) : (
          <form className="glass-card space-y-7 p-8" onSubmit={submit}>
            <section>
              <h2 className="mb-4 flex items-center gap-2 text-lg font-semibold"><Database size={18} />{t('init.postgres')}</h2>
              <div className="grid gap-4 sm:grid-cols-2">
                <Field label={t('init.host')}><input className="apple-input w-full" value={fields.pg_host} onChange={(event) => update('pg_host', event.target.value)} required autoComplete="off" /></Field>
                <Field label={t('init.port')}><input className="apple-input w-full" type="number" min="1" max="65535" value={fields.pg_port} onChange={(event) => update('pg_port', event.target.value)} required /></Field>
                <Field label={t('init.database')}><input className="apple-input w-full" value={fields.pg_database} onChange={(event) => update('pg_database', event.target.value)} required autoComplete="off" /></Field>
                <Field label={t('init.user')}><input className="apple-input w-full" value={fields.pg_user} onChange={(event) => update('pg_user', event.target.value)} required autoComplete="username" /></Field>
                <Field label={t('init.password')}><input className="apple-input w-full" type="password" value={fields.pg_password} onChange={(event) => update('pg_password', event.target.value)} required autoComplete="new-password" /></Field>
                <Field label={t('init.sslMode')}><select className="apple-input w-full" value={fields.pg_sslmode} onChange={(event) => update('pg_sslmode', event.target.value)}>{sslModes.map((mode) => <option key={mode} value={mode}>{t(`enums.sslMode.${mode}`)}</option>)}</select></Field>
              </div>
              <Button className="mt-4" variant="secondary" type="button" loading={busy === 'postgres'} onClick={() => testConnection('postgres')}>{t('init.testPostgres')}</Button>
            </section>
            <section>
              <h2 className="mb-4 flex items-center gap-2 text-lg font-semibold"><Server size={18} />{t('init.redis')}</h2>
              <div className="grid gap-4 sm:grid-cols-2">
                <Field label={t('init.host')}><input className="apple-input w-full" value={fields.redis_host} onChange={(event) => update('redis_host', event.target.value)} required autoComplete="off" /></Field>
                <Field label={t('init.port')}><input className="apple-input w-full" type="number" min="1" max="65535" value={fields.redis_port} onChange={(event) => update('redis_port', event.target.value)} required /></Field>
                <Field label={t('init.dbIndex')}><input className="apple-input w-full" type="number" min="0" max="255" value={fields.redis_database} onChange={(event) => update('redis_database', event.target.value)} required /></Field>
                <Field label={t('init.passwordOptional')}><input className="apple-input w-full" type="password" value={fields.redis_password} onChange={(event) => update('redis_password', event.target.value)} autoComplete="new-password" /></Field>
              </div>
              <label className="mt-4 flex items-center gap-2 text-sm"><input type="checkbox" checked={fields.redis_tls} onChange={(event) => update('redis_tls', event.target.checked)} />{t('init.redisTls')}</label>
              <Button className="mt-4" variant="secondary" type="button" loading={busy === 'redis'} onClick={() => testConnection('redis')}>{t('init.testRedis')}</Button>
            </section>
            <section>
              <h2 className="mb-4 text-lg font-semibold">{t('init.adminAccount')}</h2>
              <div className="grid gap-4 sm:grid-cols-2">
                <Field label={t('init.adminEmail')}><input className="apple-input w-full" type="email" value={fields.admin_email} onChange={(event) => update('admin_email', event.target.value)} required autoComplete="email" /></Field>
                <Field label={t('init.adminPassword')} hint={t('passwordPolicy.hint')}><input className="apple-input w-full" type="password" minLength={6} maxLength={32} value={fields.admin_password} onChange={(event) => update('admin_password', event.target.value)} required autoComplete="new-password" /></Field>
                <Field label={t('init.confirmPassword')}><input className="apple-input w-full" type="password" minLength={6} maxLength={32} value={fields.admin_password_confirm} onChange={(event) => update('admin_password_confirm', event.target.value)} required autoComplete="new-password" /></Field>
              </div>
            </section>
            {message && <p className="text-sm text-apple-red" role="status">{message}</p>}
            <Button type="submit" loading={busy === 'submit'}>{t('init.submit')}</Button>
          </form>
        )}
      </div>
    </main>
  )
}

function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="block">
      <span className="apple-label">{label}</span>
      {hint ? <span className="mb-1 block text-xs text-muted-foreground">{hint}</span> : null}
      {children}
    </label>
  )
}
