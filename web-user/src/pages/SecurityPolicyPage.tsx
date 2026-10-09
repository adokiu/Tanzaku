import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import apiClient from '@/api/client'
import { PageListHeader } from '@/components/PageListHeader'
import { translateApiError } from '@/i18n/apiError'
import { PolicySummaryTags } from '@/pages/security/PolicySummaryTags'
import type { GuardPolicy } from '@/pages/security/policyShared'
import './security/security.css'

type SecurityPolicy = {
  safe_mode: boolean
  guard_policy: GuardPolicy
  notes: string
}

export default function SecurityPolicyPage() {
  const { t } = useTranslation()
  const query = useQuery({
    queryKey: ['me-security-policy'],
    queryFn: async () => (await apiClient.get('/v1/me/security/policy')).data as SecurityPolicy,
  })

  return (
    <section className="page-container">
      <PageListHeader
        title={t('pages.securityPolicy')}
        actions={(
          <Link className="text-sm text-primary hover:underline" to="/security/events">
            {t('pages.securityEvents')}
          </Link>
        )}
      />
      {query.isError ? (
        <p className="page-card p-5 text-sm text-destructive">{translateApiError(t, query.error)}</p>
      ) : query.isPending ? (
        <p className="page-card p-5 text-sm text-muted-foreground">{t('common.loading')}</p>
      ) : (
        <div className="page-card flex flex-col gap-4 p-5">
          <div className="text-sm text-muted-foreground">{query.data.notes}</div>
          <div className="flex flex-wrap items-center gap-2 text-sm">
            <span className="text-muted-foreground">{t('security.safeMode')}</span>
            <span className="data-table-tag data-table-tag--neutral">
              {query.data.safe_mode ? t('common.enabled') : t('common.disabled')}
            </span>
          </div>
          <div>
            <div className="mb-2 text-sm font-medium">{t('security.effectiveSummary')}</div>
            <PolicySummaryTags policy={query.data.guard_policy} />
          </div>
        </div>
      )}
    </section>
  )
}
