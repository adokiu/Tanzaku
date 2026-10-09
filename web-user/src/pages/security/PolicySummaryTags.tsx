import { useTranslation } from 'react-i18next'
import { summaryParts, type GuardPolicy } from './policyShared'

export function PolicySummaryTags({ policy }: { policy: GuardPolicy }) {
  const { t } = useTranslation()
  const parts = summaryParts(policy, t)
  if (!parts.length) {
    return <span className="text-sm text-muted-foreground">{t('common.empty')}</span>
  }
  return (
    <div className="policy-summary-tags">
      {parts.map((part) => (
        <span key={part} className="data-table-tag data-table-tag--neutral">
          {part}
        </span>
      ))}
    </div>
  )
}
