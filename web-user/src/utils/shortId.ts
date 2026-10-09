/** 表格内 UUID 只显示第一段，完整值放 title/tooltip */
export function uuidFirstSegment(id: string): string {
  const trimmed = id.trim()
  const dash = trimmed.indexOf('-')
  if (dash > 0) {
    return trimmed.slice(0, dash)
  }
  return trimmed
}
