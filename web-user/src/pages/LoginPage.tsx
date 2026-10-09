import { useEffect, useState, type FormEvent } from 'react'
import { useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { Button } from '@/components/Button'
import { SiteFooter } from '@/components/SiteFooter'
import { useAuthStore } from '@/stores/auth'
import { displaySiteTitle, useBrandingStore } from '@/stores/branding'
import { translateApiError } from '@/i18n/apiError'
import { toast } from '@/stores/toast'
import './LoginPage.css'

export default function LoginPage({ audience }: { audience: 'admin' | 'user' }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const setAccount = useAuthStore((state) => state.setAccount)
  const branding = useBrandingStore((state) => state.branding)
  const loadBranding = useBrandingStore((state) => state.load)
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [loading, setLoading] = useState(false)

  const siteTitle = displaySiteTitle(branding, t('login.fallbackTitle'))
  const siteSubtitle = branding?.site_subtitle?.trim() || t('login.subtitle')

  useEffect(() => {
    void loadBranding()
  }, [loadBranding])

  useEffect(() => {
    document.title = siteTitle
  }, [siteTitle])

  async function handleLogin(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setLoading(true)
    try {
      const response = await apiClient.post('/auth/login', { email, password })
      const data = response.data as { user_id: string; email: string; role: 'admin' | 'user'; csrf_token: string }
      setAccount({
        user_id: data.user_id,
        email: data.email,
        role: data.role,
        csrf_token: data.csrf_token,
      })
      navigate('/dashboard', { replace: true })
    } catch (cause) {
      toast.error(translateApiError(t, cause))
    } finally {
      setLoading(false)
    }
  }

  return (
    <main className="login-page ambient-surface">
      <section className="login-page__panel">
        <header className="login-page__brand">
          <h1 className="login-page__title">{siteTitle}</h1>
          <p className="login-page__subtitle">{siteSubtitle}</p>
        </header>
        <form className="login-page__form" onSubmit={handleLogin}>
          <label className="login-page__field">
            <span>{t('login.email')}</span>
            <input
              type="email"
              value={email}
              onChange={(event) => setEmail(event.target.value)}
              autoComplete="username"
              required
            />
          </label>
          <label className="login-page__field">
            <span>{t('login.password')}</span>
            <input
              type="password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              autoComplete="current-password"
              required
            />
          </label>
          <Button type="submit" className="login-page__submit" loading={loading}>
            {loading ? t('login.submitting') : t('login.submit')}
          </Button>
        </form>
      </section>
      <SiteFooter />
    </main>
  )
}
