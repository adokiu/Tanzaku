import type { TFunction } from 'i18next'
import type { Column } from '@/components/DataTable'
import { countryFlagUrl, getCountryDisplayName } from '@/utils/iso3166'
import { uuidFirstSegment } from '@/utils/shortId'
import './EntityListLeadingCells.css'

/** 前 4 列（ID / 状态 / 地区 / IP）锁死列宽，不随内容变化 */
export const ENTITY_LIST_FIXED_COL_COUNT = 4

export const ENTITY_LIST_COL_ID = 80
export const ENTITY_LIST_COL_STATUS = 68
export const ENTITY_LIST_COL_REGION = 56
export const ENTITY_LIST_COL_IP = 93
export const ENTITY_LIST_COL_NAME = 88

export type EntityListLeadingFields = {
  id: string
  online: boolean
  region: string
  ip: string | null | undefined
  name: string
}

/** 两页共用同一套前列定义，避免列宽/渲染不一致 */
export function buildEntityListLeadingColumns<T>(options: {
  t: TFunction
  locale: string
  nameFieldKey: string
  ipColumnKey: string
  pick: (row: T) => EntityListLeadingFields
}): Column<T>[] {
  const { t, locale, nameFieldKey, ipColumnKey, pick } = options
  return [
    {
      key: 'id',
      title: t('fields.id'),
      width: ENTITY_LIST_COL_ID,
      fixedWidth: true,
      render: (row) => <EntityIdCell id={pick(row).id} />,
    },
    {
      key: 'online',
      title: t('fields.status'),
      width: ENTITY_LIST_COL_STATUS,
      fixedWidth: true,
      render: (row) => {
        const { online } = pick(row)
        return (
          <EntityStatusCell
            online={online}
            onlineLabel={t('common.online')}
            offlineLabel={t('common.offline')}
          />
        )
      },
    },
    {
      key: 'region',
      title: t('fields.region'),
      width: ENTITY_LIST_COL_REGION,
      fixedWidth: true,
      render: (row) => <EntityRegionFlagCell code={pick(row).region} locale={locale} />,
    },
    {
      key: ipColumnKey,
      title: t('fields.ip_address'),
      width: ENTITY_LIST_COL_IP,
      fixedWidth: true,
      render: (row) => <EntityIpCell value={pick(row).ip} />,
    },
    {
      key: 'name',
      title: t(`fields.${nameFieldKey}`),
      width: ENTITY_LIST_COL_NAME,
      render: (row) => <EntityNameCell name={pick(row).name} />,
    },
  ]
}

export function EntityStatusCell({
  online,
  onlineLabel,
  offlineLabel,
}: {
  online: boolean
  onlineLabel: string
  offlineLabel: string
}) {
  return (
    <span className="entity-list-cell entity-list-cell--fixed entity-list-cell--status data-table-cell">
      <span
        className={`data-table-tag entity-list-cell__status-tag ${online ? 'data-table-tag--online' : 'data-table-tag--offline'}`}
      >
        {online ? onlineLabel : offlineLabel}
      </span>
    </span>
  )
}

export function EntityIdCell({ id, title }: { id: string; title?: string }) {
  return (
    <span
      className="entity-list-cell entity-list-cell--fixed entity-list-cell--id data-table-cell"
      title={title?.trim() || id}
    >
      {uuidFirstSegment(id)}
    </span>
  )
}

export function EntityRegionFlagCell({ code, locale }: { code: string; locale: string }) {
  const upper = code.trim().toUpperCase()
  if (!upper) {
    return <span className="entity-list-cell entity-list-cell--fixed entity-list-cell--region data-table-cell">—</span>
  }
  const flag = countryFlagUrl(upper)
  const name = getCountryDisplayName(upper, locale)
  return (
    <span className="entity-list-cell entity-list-cell--fixed entity-list-cell--region data-table-cell" title={name}>
      {flag ? <img src={flag} alt="" className="entity-list-cell__flag" loading="lazy" /> : upper}
    </span>
  )
}

export function EntityIpCell({ value }: { value: string | null | undefined }) {
  const ip = value?.trim() ?? ''
  if (!ip) {
    return <span className="entity-list-cell entity-list-cell--fixed entity-list-cell--ip data-table-cell">—</span>
  }
  return (
    <span className="entity-list-cell entity-list-cell--fixed entity-list-cell--ip data-table-cell" title={ip}>
      {ip}
    </span>
  )
}

export function EntityNameCell({ name }: { name: string }) {
  const text = name.trim() || '—'
  return (
    <span className="entity-list-cell entity-list-cell--name data-table-cell" title={text}>
      {text}
    </span>
  )
}
