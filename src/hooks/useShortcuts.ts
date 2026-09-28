import { useEffect } from 'react'

/** §12.6 — shortcuts are suppressed whenever an input has the caret. */
const EDITABLE = new Set(['INPUT', 'TEXTAREA', 'SELECT'])

export type ShortcutHandler = (event: KeyboardEvent) => void

export type Shortcut = {
  /** `ctrl+a`, `esc`, `delete`, `space`, `shift` — matched on the pressed key. */
  key: string
  ctrl?: boolean
  shift?: boolean
  run: ShortcutHandler
}

/**
 * §10.3 `useShortcuts(bindings)` — the global key handler, with the I-beam
 * guard of §12.6 ("any input focused -> shortcuts suppressed"). The guard is
 * here, once, so no individual shortcut re-implements it.
 */
export function useShortcuts(bindings: Shortcut[]) {
  useEffect(() => {
    function handler(event: KeyboardEvent) {
      const target = event.target as HTMLElement | null
      if (target && EDITABLE.has(target.tagName)) {
        return
      }

      const pressed = event.key.toLowerCase()
      for (const binding of bindings) {
        if (binding.key !== pressed) {
          continue
        }
        if (Boolean(binding.ctrl) !== (event.ctrlKey || event.metaKey)) {
          continue
        }
        if (Boolean(binding.shift) !== event.shiftKey) {
          continue
        }
        event.preventDefault()
        binding.run(event)
        return
      }
    }

    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [bindings])
}
