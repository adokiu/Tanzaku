import { Info } from 'lucide-react'
import type { ReactNode } from 'react'

/** 策略名称 + info 图标，悬停显示介绍（不在页面正文直出说明）。 */
export function PolicyLabel({
  name,
  tip,
  trailing,
}: {
  name: string
  tip: string
  trailing?: ReactNode
}) {
  return (
    <span className="policy-label">
      <span className="policy-label__name">{name}</span>
      <span className="policy-info" tabIndex={0} aria-label={tip}>
        <Info size={14} aria-hidden />
        <span className="policy-info__tip" role="tooltip">
          {tip}
        </span>
      </span>
      {trailing}
    </span>
  )
}

export function PolicySection({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="policy-section">
      <h3 className="policy-section__title">{title}</h3>
      <div className="policy-section__body">{children}</div>
    </section>
  )
}
