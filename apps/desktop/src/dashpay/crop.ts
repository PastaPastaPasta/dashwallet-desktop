// Edit Profile's square crop of a file picture (DP4-03 D4), as plain geometry: the frame shows a
// square of side `min(width, height) / zoom` centred at (cx, cy), in the picture's own pixels
// (after its EXIF orientation, as the browser draws it and the engine crops it). The engine gets
// that square as a pixel rectangle and re-encodes only it (DP4-02 D18).

import type { CropArgs } from "../api/generated/CropArgs";

export const MIN_ZOOM = 1;
export const MAX_ZOOM = 4;
/** The keyboard's step: a tenth of the frame, and a quarter of zoom. */
export const PAN_STEP = 0.1;
export const ZOOM_STEP = 0.25;

export interface CropView {
  width: number;
  height: number;
  zoom: number;
  cx: number;
  cy: number;
}

/** The side of the square the frame shows, in picture pixels. */
export function cropSide(v: CropView): number {
  return Math.min(v.width, v.height) / v.zoom;
}

const clamp = (x: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, x));

/** The zoom within its bounds and the square inside the picture. */
export function clampView(v: CropView): CropView {
  const zoom = clamp(v.zoom, MIN_ZOOM, MAX_ZOOM);
  const half = Math.min(v.width, v.height) / zoom / 2;
  return { ...v, zoom, cx: clamp(v.cx, half, v.width - half), cy: clamp(v.cy, half, v.height - half) };
}

/** The whole shorter side, centred: what the engine would cut without a crop. */
export function initialView(width: number, height: number): CropView {
  return { width, height, zoom: MIN_ZOOM, cx: width / 2, cy: height / 2 };
}

/** The picture dragged by (dx, dy) frame pixels in a `frame`-pixel frame: the square moves the
 * other way. */
export function panBy(v: CropView, dx: number, dy: number, frame: number): CropView {
  const perPixel = cropSide(v) / frame;
  return clampView({ ...v, cx: v.cx - dx * perPixel, cy: v.cy - dy * perPixel });
}

/** Zoomed to `zoom` about the square's centre. */
export function zoomTo(v: CropView, zoom: number): CropView {
  return clampView({ ...v, zoom });
}

/** The square as whole pixels inside the picture, at least one pixel wide. */
export function cropRect(v: CropView): CropArgs {
  const side = Math.max(1, Math.min(Math.floor(cropSide(v)), v.width, v.height));
  const x = clamp(Math.round(v.cx - side / 2), 0, v.width - side);
  const y = clamp(Math.round(v.cy - side / 2), 0, v.height - side);
  return { x, y, width: side, height: side };
}

/** Where the picture sits in a `frame`-pixel frame (CSS pixels): its scale and offset. */
export function placement(v: CropView, frame: number): { left: number; top: number; width: number; height: number } {
  const scale = frame / cropSide(v);
  return { left: frame / 2 - v.cx * scale, top: frame / 2 - v.cy * scale, width: v.width * scale, height: v.height * scale };
}
