// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

/**
 * 页面底色 = 主题色（与 style.css 的 `--bg` 同值）。
 *
 * 地址栏 / 状态栏跟随主题是移动端唯一的系统级取色入口，所以这里与
 * `docs/design/ui-redesign` 的令牌表保持一一对应：浅色 B = #F4F5F7，深色 A = #0B0E13。
 */
export const THEME_META_COLORS = {
  light: '#f4f5f7',
  dark: '#0b0e13',
} as const

/** 系统偏好（跟随系统时使用的唯一外部输入）。 */
export function systemPrefersDark(): boolean {
  return (
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-color-scheme: dark)').matches
  )
}

/** 把主题设置解析成"当前是否深色"。 */
export function resolveIsDarkTheme(
  theme: 'system' | 'light' | 'dark',
  prefersDark: boolean = systemPrefersDark()
): boolean {
  if (theme === 'system') return prefersDark
  return theme === 'dark'
}

/**
 * 应用主题到文档根节点。
 *
 * 只切 `html.dark` + `data-theme` + `color-scheme` 和底色，
 * 不改任何布局属性 —— 因此切换主题时页面不会重排、不会闪白。
 */
export function applyThemeToDocument(isDarkMode: boolean) {
  const root = document.documentElement
  const background = isDarkMode ? THEME_META_COLORS.dark : THEME_META_COLORS.light

  root.classList.toggle('dark', isDarkMode)
  root.dataset.theme = isDarkMode ? 'dark' : 'light'
  root.style.colorScheme = isDarkMode ? 'dark' : 'light'
  root.style.backgroundColor = background

  document.getElementById('theme-color')?.setAttribute('content', background)
}
