import { CellTooltip } from '@/components/CellTooltip/CellTooltip'
import { getOSImage } from '@/utils/osImageHelper'
import './ClientSystemCell.css'

export function ClientSystemCell({
  os,
  arch,
}: {
  os?: string | null
  arch?: string | null
}) {
  const osLabel = os?.trim() || ''
  if (!osLabel) {
    return <span className="data-table-cell">—</span>
  }
  const tip = [osLabel, arch?.trim()].filter(Boolean).join(' ')
  return (
    <CellTooltip tip={tip} as="div" className="client-system-cell data-table-cell">
      <img src={getOSImage(osLabel)} alt="" className="client-system-cell__icon" loading="lazy" />
      <span className="client-system-cell__text">
        <span className="client-system-cell__os">{osLabel}</span>
        {arch?.trim() ? <span className="client-system-cell__arch">{arch.trim()}</span> : null}
      </span>
    </CellTooltip>
  )
}
