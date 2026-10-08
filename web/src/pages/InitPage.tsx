import { useEffect, useState, type FormEvent, type ReactNode } from 'react'
import { useNavigate } from 'react-router-dom'
import { Check, Database, Server } from 'lucide-react'
import apiClient from '@/api/client'
import { Button } from '@/components/Button'

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

export default function InitPage() {
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
      .catch(() => setMessage('无法连接 board'))
      .finally(() => setChecking(false))
  }, [navigate])

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
    setMessage('正在测试连接')
    try {
      await apiClient.post(`/init/test/${kind}`, { [kind]: kind === 'postgres' ? postgresPayload() : redisPayload() })
      setMessage(kind === 'postgres' ? 'PostgreSQL 连接成功' : 'Redis 连接成功')
    } catch (cause) {
      setMessage(errorMessage(cause))
    } finally {
      setBusy(null)
    }
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (fields.admin_password !== fields.admin_password_confirm) {
      setMessage('两次输入的管理员密码不一致')
      return
    }
    setBusy('submit')
    setMessage('正在初始化')
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
      setMessage(errorMessage(cause))
    } finally {
      setBusy(null)
    }
  }

  if (checking) {
    return <main className="min-h-screen flex items-center justify-center bg-background"><p className="text-sm text-muted-foreground">正在检查初始化状态</p></main>
  }

  return (
    <main className="min-h-screen bg-background flex items-center justify-center p-6">
      <div className="w-full max-w-2xl">
        <header className="mb-8 text-center">
          <h1 className="mb-2 text-3xl font-display font-semibold tracking-tight">Tanzaku 初始化</h1>
          <p className="text-sm text-muted-foreground">配置 PostgreSQL、Redis 和首个管理员账户</p>
        </header>
        {finished ? (
          <section className="glass-card p-8 text-center">
            <Check className="mx-auto mb-3 text-apple-green" size={32} />
            <h2 className="text-lg font-semibold">初始化完成</h2>
            <p className="mt-2 text-sm text-muted-foreground">正在前往管理员登录页面</p>
          </section>
        ) : (
          <form className="glass-card space-y-7 p-8" onSubmit={submit}>
            <section>
              <h2 className="mb-4 flex items-center gap-2 text-lg font-semibold"><Database size={18} />PostgreSQL</h2>
              <div className="grid gap-4 sm:grid-cols-2">
                <Field label="主机"><input className="apple-input w-full" value={fields.pg_host} onChange={(event) => update('pg_host', event.target.value)} required autoComplete="off" /></Field>
                <Field label="端口"><input className="apple-input w-full" type="number" min="1" max="65535" value={fields.pg_port} onChange={(event) => update('pg_port', event.target.value)} required /></Field>
                <Field label="数据库"><input className="apple-input w-full" value={fields.pg_database} onChange={(event) => update('pg_database', event.target.value)} required autoComplete="off" /></Field>
                <Field label="用户"><input className="apple-input w-full" value={fields.pg_user} onChange={(event) => update('pg_user', event.target.value)} required autoComplete="username" /></Field>
                <Field label="密码"><input className="apple-input w-full" type="password" value={fields.pg_password} onChange={(event) => update('pg_password', event.target.value)} required autoComplete="new-password" /></Field>
                <Field label="SSL 模式"><select className="apple-input w-full" value={fields.pg_sslmode} onChange={(event) => update('pg_sslmode', event.target.value)}><option value="prefer">prefer</option><option value="require">require</option><option value="verify-ca">verify-ca</option><option value="verify-full">verify-full</option><option value="disable">disable</option></select></Field>
              </div>
              <Button className="mt-4" variant="secondary" type="button" loading={busy === 'postgres'} onClick={() => testConnection('postgres')}>测试 PostgreSQL 连接</Button>
            </section>
            <section>
              <h2 className="mb-4 flex items-center gap-2 text-lg font-semibold"><Server size={18} />Redis</h2>
              <div className="grid gap-4 sm:grid-cols-2">
                <Field label="主机"><input className="apple-input w-full" value={fields.redis_host} onChange={(event) => update('redis_host', event.target.value)} required autoComplete="off" /></Field>
                <Field label="端口"><input className="apple-input w-full" type="number" min="1" max="65535" value={fields.redis_port} onChange={(event) => update('redis_port', event.target.value)} required /></Field>
                <Field label="数据库编号"><input className="apple-input w-full" type="number" min="0" max="255" value={fields.redis_database} onChange={(event) => update('redis_database', event.target.value)} required /></Field>
                <Field label="密码（可选）"><input className="apple-input w-full" type="password" value={fields.redis_password} onChange={(event) => update('redis_password', event.target.value)} autoComplete="new-password" /></Field>
              </div>
              <label className="mt-4 flex items-center gap-2 text-sm"><input type="checkbox" checked={fields.redis_tls} onChange={(event) => update('redis_tls', event.target.checked)} />Redis TLS</label>
              <Button className="mt-4" variant="secondary" type="button" loading={busy === 'redis'} onClick={() => testConnection('redis')}>测试 Redis 连接</Button>
            </section>
            <section>
              <h2 className="mb-4 text-lg font-semibold">管理员账户</h2>
              <div className="grid gap-4 sm:grid-cols-2">
                <Field label="邮箱"><input className="apple-input w-full" type="email" value={fields.admin_email} onChange={(event) => update('admin_email', event.target.value)} required autoComplete="email" /></Field>
                <Field label="密码（至少 12 位）"><input className="apple-input w-full" type="password" minLength={12} value={fields.admin_password} onChange={(event) => update('admin_password', event.target.value)} required autoComplete="new-password" /></Field>
                <Field label="确认密码"><input className="apple-input w-full" type="password" minLength={12} value={fields.admin_password_confirm} onChange={(event) => update('admin_password_confirm', event.target.value)} required autoComplete="new-password" /></Field>
              </div>
            </section>
            {message && <p className="text-sm text-apple-red" role="status">{message}</p>}
            <Button type="submit" loading={busy === 'submit'}>初始化 board</Button>
          </form>
        )}
      </div>
    </main>
  )
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return <label className="block"><span className="apple-label">{label}</span>{children}</label>
}

function errorMessage(error: unknown): string {
  if (typeof error === 'object' && error !== null && 'response' in error) {
    const data = (error as { response?: { data?: { error?: string } } }).response?.data
    if (data?.error) return data.error
  }
  return '请求失败，请检查服务状态和连接配置'
}
