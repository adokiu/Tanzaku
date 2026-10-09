import { useState, type FormEvent } from 'react'
import { useTranslation } from 'react-i18next'
import apiClient from '@/api/client'
import { Button } from '@/components/Button'
import { PageListHeader } from '@/components/PageListHeader'
import { translateApiError } from '@/i18n/apiError'
import { useAuthStore } from '@/stores/auth'
import { toast } from '@/stores/toast'
import './AccountSecurityPage.css'

export default function AccountSecurityPage() {
  const { t } = useTranslation()
  const account = useAuthStore((state) => state.account)
  const setAccount = useAuthStore((state) => state.setAccount)

  const [currentPassword, setCurrentPassword] = useState('')
  const [newPassword, setNewPassword] = useState('')
  const [confirmPassword, setConfirmPassword] = useState('')
  const [passwordLoading, setPasswordLoading] = useState(false)

  const [emailPassword, setEmailPassword] = useState('')
  const [newEmail, setNewEmail] = useState('')
  const [emailLoading, setEmailLoading] = useState(false)

  async function handleChangePassword(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (newPassword !== confirmPassword) {
      toast.error(t('accountSecurity.passwordMismatch'))
      return
    }
    setPasswordLoading(true)
    try {
      await apiClient.post('/v1/me/password', {
        current_password: currentPassword,
        new_password: newPassword,
      })
      setCurrentPassword('')
      setNewPassword('')
      setConfirmPassword('')
      toast.success(t('accountSecurity.passwordOk'))
    } catch (cause) {
      toast.error(translateApiError(t, cause))
    } finally {
      setPasswordLoading(false)
    }
  }

  async function handleChangeEmail(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setEmailLoading(true)
    try {
      const response = await apiClient.post('/v1/me/email', {
        current_password: emailPassword,
        new_email: newEmail,
      })
      const data = response.data as { email: string }
      if (account) {
        setAccount({ ...account, email: data.email })
      }
      setEmailPassword('')
      setNewEmail('')
      toast.success(t('accountSecurity.emailOk'))
    } catch (cause) {
      toast.error(translateApiError(t, cause))
    } finally {
      setEmailLoading(false)
    }
  }

  return (
    <section className="page-container account-security">
      <PageListHeader titleKey="nav.accountSecurity" />
      <p className="account-security__email">
        {t('accountSecurity.currentEmail')}
        <strong>{account?.email ?? '—'}</strong>
      </p>

      <div className="account-security__grid">
        <form className="page-card account-security__card" onSubmit={handleChangePassword}>
          <h2 className="account-security__card-title">{t('accountSecurity.changePassword')}</h2>
          <label className="account-security__field">
            <span>{t('accountSecurity.currentPassword')}</span>
            <input
              className="apple-input"
              type="password"
              value={currentPassword}
              onChange={(event) => setCurrentPassword(event.target.value)}
              autoComplete="current-password"
              required
            />
          </label>
          <label className="account-security__field">
            <span>{t('accountSecurity.newPassword')}</span>
            <input
              className="apple-input"
              type="password"
              value={newPassword}
              onChange={(event) => setNewPassword(event.target.value)}
              autoComplete="new-password"
              required
            />
          </label>
          <label className="account-security__field">
            <span>{t('accountSecurity.confirmPassword')}</span>
            <input
              className="apple-input"
              type="password"
              value={confirmPassword}
              onChange={(event) => setConfirmPassword(event.target.value)}
              autoComplete="new-password"
              required
            />
          </label>
          <Button type="submit" loading={passwordLoading}>
            {t('accountSecurity.submitPassword')}
          </Button>
        </form>

        <form className="page-card account-security__card" onSubmit={handleChangeEmail}>
          <h2 className="account-security__card-title">{t('accountSecurity.changeEmail')}</h2>
          <label className="account-security__field">
            <span>{t('accountSecurity.currentPassword')}</span>
            <input
              className="apple-input"
              type="password"
              value={emailPassword}
              onChange={(event) => setEmailPassword(event.target.value)}
              autoComplete="current-password"
              required
            />
          </label>
          <label className="account-security__field">
            <span>{t('accountSecurity.newEmail')}</span>
            <input
              className="apple-input"
              type="email"
              value={newEmail}
              onChange={(event) => setNewEmail(event.target.value)}
              autoComplete="email"
              required
            />
          </label>
          <Button type="submit" loading={emailLoading}>
            {t('accountSecurity.submitEmail')}
          </Button>
        </form>
      </div>
    </section>
  )
}
