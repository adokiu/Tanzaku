/** 与 komari-theme-naive 主题配置默认值一致；多字体用逗号分隔 */
export const DEFAULT_FONT_FAMILY = '"MiSans VF", sans-serif'
export const DEFAULT_NUMBER_FONT_FAMILY = '"TCloud Number VF", "MiSans VF", sans-serif'

export function applyTypographyVariables(options?: {
  fontFamily?: string
  numberFontFamily?: string
}) {
  const root = document.documentElement
  root.style.setProperty('--font-family', options?.fontFamily?.trim() || DEFAULT_FONT_FAMILY)
  root.style.setProperty('--font-family-number', options?.numberFontFamily?.trim() || DEFAULT_NUMBER_FONT_FAMILY)
}
