// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

import { pickServerMessages, type ServerLocale } from '../../stores/serverMessages'

/**
 * 原生模式的 Token 输入框（契约 §3.5）。
 *
 * 为什么是命令式 DOM 而不是 `.vue` 组件：401 可能发生在任何页面、任何时刻
 * （配置 bootstrap 就可能撞上），因此不能依赖某个页面被挂载；这里直接挂到 body，
 * 不引入新的全局组件，也不改变管理器模式的渲染树。
 */

export interface TokenPromptOptions {
  locale: ServerLocale
  reason?: 'unauthorized' | 'missing'
  initialToken?: string
  remember?: boolean
}

export interface TokenPromptResult {
  token: string
  remember: boolean
}

interface ActivePrompt {
  close: (result: TokenPromptResult | null) => void
}

let activePrompt: ActivePrompt | null = null

function style(el: HTMLElement, declarations: Record<string, string>): void {
  for (const [property, value] of Object.entries(declarations)) {
    el.style.setProperty(property, value)
  }
}

function createButton(label: string, primary: boolean): HTMLButtonElement {
  const button = document.createElement('button')
  button.type = 'button'
  button.textContent = label
  style(button, {
    padding: '0.5rem 1rem',
    'border-radius': '0.5rem',
    border: primary ? 'none' : '1px solid var(--border, rgba(0,0,0,0.16))',
    background: primary ? 'var(--primary, #3b82f6)' : 'transparent',
    color: primary ? '#fff' : 'var(--text, #111)',
    'font-size': '0.875rem',
    cursor: 'pointer',
    'min-width': '5rem',
  })
  return button
}

/** 弹出 Token 输入框；确认返回 Token，取消/ESC 返回 null。 */
export function promptForToken(options: TokenPromptOptions): Promise<TokenPromptResult | null> {
  if (typeof document === 'undefined') {
    return Promise.resolve(null)
  }

  // 同一时刻只保留一个对话框
  activePrompt?.close(null)

  const messages = pickServerMessages(options.locale).token

  return new Promise<TokenPromptResult | null>((resolve) => {
    const overlay = document.createElement('div')
    overlay.setAttribute('role', 'presentation')
    style(overlay, {
      position: 'fixed',
      inset: '0',
      'z-index': '4000',
      display: 'flex',
      'align-items': 'center',
      'justify-content': 'center',
      padding: '1rem',
      background: 'rgba(15, 15, 20, 0.45)',
      'backdrop-filter': 'blur(6px)',
    })

    const card = document.createElement('div')
    card.setAttribute('role', 'dialog')
    card.setAttribute('aria-modal', 'true')
    style(card, {
      width: '100%',
      'max-width': '22rem',
      padding: '1.25rem',
      'border-radius': '1rem',
      background: 'var(--background, #fff)',
      color: 'var(--text, #111)',
      border: '1px solid var(--border, rgba(0,0,0,0.08))',
      'box-shadow': '0 12px 40px rgba(0,0,0,0.28)',
      'font-family': 'inherit',
      'box-sizing': 'border-box',
    })

    const title = document.createElement('h3')
    title.textContent = messages.title
    style(title, { margin: '0 0 0.5rem', 'font-size': '1.05rem', 'font-weight': '600' })

    const desc = document.createElement('p')
    desc.textContent = options.reason === 'unauthorized' ? messages.unauthorized : messages.desc
    style(desc, {
      margin: '0 0 0.75rem',
      'font-size': '0.825rem',
      'line-height': '1.5',
      color: 'var(--text-secondary, #666)',
    })

    const input = document.createElement('input')
    input.type = 'password'
    input.autocomplete = 'off'
    input.spellcheck = false
    input.placeholder = messages.placeholder
    input.value = options.initialToken || ''
    style(input, {
      width: '100%',
      'box-sizing': 'border-box',
      padding: '0.6rem 0.75rem',
      'border-radius': '0.5rem',
      border: '1px solid var(--border, rgba(0,0,0,0.16))',
      background: 'var(--surface, rgba(0,0,0,0.03))',
      color: 'inherit',
      'font-size': '0.9rem',
      'font-family': 'inherit',
    })

    const errorLine = document.createElement('div')
    errorLine.setAttribute('role', 'alert')
    style(errorLine, {
      'min-height': '1.1rem',
      margin: '0.4rem 0 0',
      'font-size': '0.8rem',
      color: '#dc2626',
    })

    const rememberRow = document.createElement('label')
    style(rememberRow, {
      display: 'flex',
      'align-items': 'center',
      gap: '0.5rem',
      margin: '0.5rem 0 0.9rem',
      'font-size': '0.85rem',
      cursor: 'pointer',
    })

    const rememberInput = document.createElement('input')
    rememberInput.type = 'checkbox'
    rememberInput.checked = options.remember ?? false
    const rememberText = document.createElement('span')
    rememberText.textContent = messages.remember
    rememberRow.append(rememberInput, rememberText)

    const actions = document.createElement('div')
    style(actions, { display: 'flex', 'justify-content': 'flex-end', gap: '0.5rem' })

    const cancelButton = createButton(messages.cancel, false)
    const submitButton = createButton(messages.submit, true)
    actions.append(cancelButton, submitButton)

    card.append(title, desc, input, errorLine, rememberRow, actions)
    overlay.append(card)

    function cleanup(): void {
      document.removeEventListener('keydown', onDocumentKeydown, true)
      overlay.remove()
      if (activePrompt === current) {
        activePrompt = null
      }
    }

    let settled = false
    function finish(result: TokenPromptResult | null): void {
      if (settled) return
      settled = true
      cleanup()
      resolve(result)
    }

    const current: ActivePrompt = {
      close: (result) => finish(result),
    }
    activePrompt = current

    function submit(): void {
      const token = input.value.trim()
      if (!token) {
        errorLine.textContent = messages.empty
        input.focus()
        return
      }
      finish({ token, remember: rememberInput.checked })
    }

    function onDocumentKeydown(event: KeyboardEvent): void {
      if (event.key === 'Escape') {
        event.preventDefault()
        finish(null)
      }
    }

    input.addEventListener('keydown', (event) => {
      if (event.key === 'Enter') {
        event.preventDefault()
        submit()
      }
    })
    submitButton.addEventListener('click', submit)
    cancelButton.addEventListener('click', () => finish(null))
    rememberInput.addEventListener('keydown', (event) => {
      if (event.key === 'Enter') {
        event.preventDefault()
        submit()
      }
    })
    document.addEventListener('keydown', onDocumentKeydown, true)

    document.body.appendChild(overlay)
    window.setTimeout(() => input.focus(), 0)
  })
}

/** 关闭当前对话框（退出登录等场景）。 */
export function dismissTokenPrompt(): void {
  activePrompt?.close(null)
}
