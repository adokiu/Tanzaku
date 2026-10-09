import type { Column } from '@/components/DataTable'
import {
  ENTITY_LIST_COL_ID,
  ENTITY_LIST_COL_IP,
  ENTITY_LIST_COL_REGION,
  ENTITY_LIST_COL_STATUS,
} from '@/components/EntityListLeadingCells'
import { columnWeightFromListSettings, parseCssLengthToPx } from '@/utils/listColumnTrack'

export type EntityListLayoutId = 'nodes' | 'clients' | 'users' | 'tunnels' | 'plans' | 'certificates'

export const ENTITY_LIST_PAGE_MAX_WIDTH = '1800px'
export const ENTITY_LIST_COLUMN_GAP = '12px'
export const ENTITY_LIST_BORDER_RADIUS = '8px'

/** 不设固定像素宽，由 table-layout:fixed 吃满剩余空间（按列表页） */
export function entityListGrowColumnKeys(listId: EntityListLayoutId): Set<string> {
  switch (listId) {
    case 'users':
      return new Set(['email'])
    case 'nodes':
    case 'clients':
      return new Set(['name'])
    case 'tunnels':
      return new Set([])
    case 'plans':
      return new Set(['name'])
    case 'certificates':
      return new Set(['domains'])
  }
}

/** 前 4 列：ID / 状态 / 地区 / IP，宽度永不参与比例伸缩 */
export const ENTITY_LIST_LOCKED_COLUMN_PX: Record<string, number> = {
  id: ENTITY_LIST_COL_ID,
  online: ENTITY_LIST_COL_STATUS,
  region: ENTITY_LIST_COL_REGION,
  public_host: ENTITY_LIST_COL_IP,
  public_ip: ENTITY_LIST_COL_IP,
  status: 76,
}

export function isEntityListLockedColumn(key: string): boolean {
  return key in ENTITY_LIST_LOCKED_COLUMN_PX
}

const COLUMN_WIDTHS_NODES: Record<string, string> = {
  id: '80px',
  online: '68px',
  region: '56px',
  public_host: '93px',
  name: 'minmax(160px, 1.2fr)',
  cpu: '116px',
  memory: '116px',
  bandwidth: '140px',
  tunnel_count: '72px',
  connections: '72px',
  last_seen_at: '156px',
}

const COLUMN_WIDTHS_USERS: Record<string, string> = {
  id: '80px',
  email: 'minmax(200px, 1fr)',
  status: '76px',
  role: '96px',
  subscription: '128px',
  traffic_used: '128px',
  traffic_total: '96px',
  expires_at: '168px',
  balance: '88px',
  created_at: '168px',
  actions: '168px',
}

const COLUMN_WIDTHS_TUNNELS: Record<string, string> = {
  id: '80px',
  user_id: '96px',
  client_id: '96px',
  status: '88px',
  carrier: '72px',
  remote_port: '150px',
  target: 'minmax(120px, 0.5fr)',
  traffic_speed: '140px',
  traffic_total: '100px',
  actions: '168px',
}

const COLUMN_WIDTHS_CERTIFICATES: Record<string, string> = {
  id: '80px',
  owner: '220px',
  domains: 'minmax(180px, 1fr)',
  issuer: '200px',
  not_after: '120px',
  source: '100px',
  actions: '168px',
}

const COLUMN_WIDTHS_PLANS: Record<string, string> = {
  name: 'minmax(120px, 0.5fr)',
  node_groups: '140px',
  traffic_quota: '120px',
  traffic_period: '140px',
  speed_limit_mbps: '88px',
  max_tunnels: '72px',
  enabled: '72px',
  actions: '168px',
}

const COLUMN_WIDTHS_CLIENTS: Record<string, string> = {
  id: '80px',
  online: '68px',
  region: '56px',
  public_ip: '93px',
  name: 'minmax(88px, 0.6fr)',
  user_email: 'minmax(160px, 1fr)',
  system: '200px',
  traffic_in: '100px',
  traffic_out: '100px',
  traffic_speed: '140px',
  tunnel_count: '72px',
  last_seen_at: '168px',
}

const NAME_COLUMN_MIN_PX = 88

function entityListGrowMinPx(listId: EntityListLayoutId, columnKey: string): number {
  if (listId === 'users' && columnKey === 'email') return 200
  if (listId === 'certificates' && columnKey === 'domains') return 180
  if (listId === 'nodes' && columnKey === 'name') return 160
  return NAME_COLUMN_MIN_PX
}

/** 三枚文字按钮 + gap，仅用于 table-layout:fixed 分栏（避免列宽 1% 把内容挤出卡片） */
export const ENTITY_LIST_ACTIONS_COLUMN_PX = 168

export function isEntityListActionsColumn(key: string): boolean {
  return key === 'actions' || key === 'action'
}

export function columnWidthsForEntityList(listId: EntityListLayoutId): Record<string, string> {
  if (listId === 'nodes') return COLUMN_WIDTHS_NODES
  if (listId === 'users') return COLUMN_WIDTHS_USERS
  if (listId === 'tunnels') return COLUMN_WIDTHS_TUNNELS
  if (listId === 'plans') return COLUMN_WIDTHS_PLANS
  if (listId === 'certificates') return COLUMN_WIDTHS_CERTIFICATES
  return COLUMN_WIDTHS_CLIENTS
}

export function entityListPageMaxWidthPx(): number {
  return parseCssLengthToPx(ENTITY_LIST_PAGE_MAX_WIDTH) || 1800
}

/** 各列最小宽度之和：窄屏时表格不低于此宽度，在容器内横向滑动 */
export function entityListMinTableWidth(
  columns: Pick<Column<unknown>, 'key' | 'width'>[],
  listId: EntityListLayoutId,
): number {
  const columnWidths = columnWidthsForEntityList(listId)
  const growKeys = entityListGrowColumnKeys(listId)
  let sum = 0
  for (const col of columns) {
    const locked = ENTITY_LIST_LOCKED_COLUMN_PX[col.key]
    if (locked != null) {
      sum += locked
      continue
    }
    if (isEntityListActionsColumn(col.key)) {
      sum += ENTITY_LIST_ACTIONS_COLUMN_PX
      continue
    }
    if (growKeys.has(col.key)) {
      sum += entityListGrowMinPx(listId, col.key)
      continue
    }
    sum += columnWeightFromListSettings(
      col.key,
      columnWidths,
      typeof col.width === 'number' ? col.width : 72,
    )
  }
  return sum + 32
}

export function distributeEntityListColumns(
  columns: Pick<Column<unknown>, 'key' | 'width'>[],
  listId: EntityListLayoutId,
  tableWidthPx?: number,
): Record<string, number> {
  const columnWidths = columnWidthsForEntityList(listId)
  const pageMax = entityListPageMaxWidthPx()
  const measured = tableWidthPx != null && tableWidthPx > 0 ? tableWidthPx : pageMax
  const naturalMin = entityListMinTableWidth(columns, listId)
  // 容器更窄时仍按最小自然宽度分配，避免列被压扁；由滚动容器横向滑动
  const contentWidth = Math.max(naturalMin, Math.min(measured, pageMax))

  const lockedSum = columns.reduce((sum, col) => {
    const px = ENTITY_LIST_LOCKED_COLUMN_PX[col.key]
    return px != null ? sum + px : sum
  }, 0)

  const growKeys = entityListGrowColumnKeys(listId)
  const growReserve = columns.reduce(
    (sum, col) => (growKeys.has(col.key) ? sum + entityListGrowMinPx(listId, col.key) : sum),
    0,
  )

  const flexColumns = columns.filter(
    (col) =>
      !growKeys.has(col.key) &&
      !isEntityListLockedColumn(col.key) &&
      !isEntityListActionsColumn(col.key),
  )
  const actionsCol = columns.find((col) => isEntityListActionsColumn(col.key))
  const actionsReserve = actionsCol ? ENTITY_LIST_ACTIONS_COLUMN_PX : 0
  const flexBudget = Math.max(160, contentWidth - lockedSum - growReserve - actionsReserve)

  const weights = flexColumns.map((col) =>
    columnWeightFromListSettings(
      col.key,
      columnWidths,
      typeof col.width === 'number' ? col.width : 72,
    ),
  )
  const weightSum = weights.reduce((sum, w) => sum + w, 0) || 1
  const allocatedFlex = weights.map((w) => Math.floor((w / weightSum) * flexBudget))
  let usedFlex = allocatedFlex.reduce((sum, w) => sum + w, 0)
  const remainder = flexBudget - usedFlex
  if (remainder !== 0 && allocatedFlex.length > 0) {
    allocatedFlex[allocatedFlex.length - 1] =
      (allocatedFlex[allocatedFlex.length - 1] ?? 0) + remainder
  }

  const map: Record<string, number> = {}
  let flexIndex = 0
  for (const col of columns) {
    if (growKeys.has(col.key)) continue
    if (isEntityListActionsColumn(col.key)) continue
    const locked = ENTITY_LIST_LOCKED_COLUMN_PX[col.key]
    if (locked != null) {
      map[col.key] = locked
      continue
    }
    map[col.key] = allocatedFlex[flexIndex] ?? 72
    flexIndex += 1
  }

  if (actionsCol) {
    map[actionsCol.key] = ENTITY_LIST_ACTIONS_COLUMN_PX
  }

  return map
}
