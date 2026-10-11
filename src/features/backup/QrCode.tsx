import { useEffect, useRef } from 'react'

/** Modules of padding around the code. The QR spec requires 4 on every side. */
const QUIET_ZONE = 4

/** CSS size of the drawn square. The backing store is scaled by the DPR. */
const CSS_SIZE = 208

/**
 * §11.3's QR code. `matrix` arrives row-major with `true` = a dark module, built
 * in Rust (§7.8) so the encoder lives in exactly one place; this only paints.
 *
 * Two deliberate exceptions to §19.1, both functional rather than decorative:
 * the square is **not** rounded (rounding would clip the finder patterns and
 * break scanning) and its quiet zone is white (a scanner needs that contrast —
 * an inverted or translucent code is unreliable). The card around it keeps the
 * MD3 shape.
 */
export function QrCode({ matrix, label }: { matrix: boolean[][]; label: string }) {
  const ref = useRef<HTMLCanvasElement | null>(null)

  useEffect(() => {
    const canvas = ref.current
    if (!canvas) {
      return
    }
    const rows = matrix.length
    const cols = rows > 0 ? (matrix[0]?.length ?? 0) : 0
    if (rows === 0 || cols === 0) {
      return
    }

    const modules = cols + QUIET_ZONE * 2
    const dpr = window.devicePixelRatio || 1
    const backing = Math.round(CSS_SIZE * dpr)
    canvas.width = backing
    canvas.height = backing
    canvas.style.width = `${CSS_SIZE}px`
    canvas.style.height = `${CSS_SIZE}px`

    const ctx = canvas.getContext('2d')
    if (!ctx) {
      return
    }
    ctx.fillStyle = '#ffffff'
    ctx.fillRect(0, 0, backing, backing)

    const scale = backing / modules
    ctx.fillStyle = '#000000'
    for (let row = 0; row < rows; row += 1) {
      for (let col = 0; col < cols; col += 1) {
        if (!matrix[row]?.[col]) {
          continue
        }
        ctx.fillRect((col + QUIET_ZONE) * scale, (row + QUIET_ZONE) * scale, scale, scale)
      }
    }
  }, [matrix])

  return <canvas ref={ref} className="wifi-qr" role="img" aria-label={label} />
}
