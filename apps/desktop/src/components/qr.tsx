// A QR code drawn from dw-uri's module matrix (`qrMatrix`, level L): one SVG path, dark modules
// on a light square with the 4-module quiet zone scanners need, in either appearance. No centre
// badge: at level L it would break scanning (UX-SPEC §4.8).

import type { QrMatrix } from "../api/generated/QrMatrix";

const QUIET = 4;

/** The dark modules as one path of 1×1 squares, offset by the quiet zone. */
export function qrPath(m: QrMatrix): string {
  let d = "";
  for (let y = 0; y < m.size; y++)
    for (let x = 0; x < m.size; x++) if (m.modules[y * m.size + x]) d += `M${x + QUIET} ${y + QUIET}h1v1h-1z`;
  return d;
}

export function QrCode({ matrix, size, label, testId }: { matrix: QrMatrix; size: number; label: string; testId?: string }) {
  const side = matrix.size + 2 * QUIET;
  return (
    <svg className="qr" width={size} height={size} viewBox={`0 0 ${side} ${side}`} role="img" aria-label={label} data-testid={testId} shapeRendering="crispEdges">
      <rect className="qr-light" width={side} height={side} />
      <path className="qr-dark" d={qrPath(matrix)} />
    </svg>
  );
}
