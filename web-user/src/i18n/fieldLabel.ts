import type { TFunction } from 'i18next'

export function translateField(t: TFunction, key: string): string {
  return t(`fields.${key}`, { defaultValue: humanizeFieldKey(key) })
}

function humanizeFieldKey(key: string): string {
  return key.replace(/_/g, ' ')
}
