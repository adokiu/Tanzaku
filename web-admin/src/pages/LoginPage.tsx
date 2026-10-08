import { useState, type FormEvent } from 'react'
import { useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { LogIn } from 'lucide-react'
import apiClient from '@/api/client'
import { Button } from '@/components/Button'
import { useAuthStore } from '@/stores/auth'
import { translateApiError } from '@/i18n/apiError'

export default function LoginPage({ audience }: { audience: 'admin' | 'user' }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const setAccount = useAuthStore((state) => state.setAccount)
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')

  async function handleLogin(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setLoading(true)
    setError('')
    try {
      const response = await apiClient.post('/auth/login', { email, password })
      const data = response.data as { user_id: string; email: string; role: 'admin' | 'user'; csrf_token: string }
      setAccount({
        user_id: data.user_id,
        email: data.email,
        role: data.role,
        csrf_token: data.csrf_token,
      })
      navigate(audience === 'admin' ? '/dashboard' : '/overview', { replace: true })
    } catch (cause) {
      setError(translateApiError(t, cause))
    } finally {
      setLoading(false)
    }
  }

  return (
    <main className="min-h-screen bg-background flex items-center justify-center p-6">
      <div className="w-full max-w-sm">
        <header className="mb-10 text-center">
          <div className="mx-auto mb-5 flex h-14 w-14 items-center justify-center rounded-2xl bg-apple-blue/10"><LogIn className="text-apple-blue" size={28} /></div>
          <h1 className="mb-1 text-2xl font-display font-semibold tracking-tight">{t('login.title')}</h1>
          <p className="text-sm text-muted-foreground">{t('login.subtitle')}</p>
        </header>
        <form onSubmit={handleLogin} className="glass-card space-y-5 p-8">
          <label className="block">
            <span className="apple-label">{t('login.email')}</span>
            <input className="apple-input w-full" type="email" value={email} onChange={(event) => setEmail(event.target.value)} autoComplete="username" required />
          </label>
          <label className="block">
            <span className="apple-label">{t('login.password')}</span>
            <input className="apple-input w-full" type="password" value={password} onChange={(event) => setPassword(event.target.value)} autoComplete="current-password" required />
          </label>
          {error && <div role="alert" className="rounded-xl bg-apple-red/10 px-4 py-3 text-sm text-apple-red">{error}</div>}
          <Button type="submit" className="w-full" loading={loading}>{loading ? t('login.submitting') : t('login.submit')}</Button>
        </form>
      </div>
    </main>
  )
}
