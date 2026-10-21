// The square crop's geometry (DP4-03 D4): the frame's square stays inside the picture at any
// zoom, a drag moves it the other way, and the engine gets whole pixels.

import { describe, expect, it } from "vitest";
import { clampView, cropRect, initialView, MAX_ZOOM, panBy, placement, zoomTo } from "./crop";

describe("crop (D4)", () => {
  it("starts at the whole shorter side, centred", () => {
    const v = initialView(400, 300);
    expect(cropRect(v)).toEqual({ x: 50, y: 0, width: 300, height: 300 });
    expect(placement(v, 240)).toEqual({ left: -40, top: 0, width: 320, height: 240 });
  });

  it("keeps the zoom in 1–4× and the square inside the picture", () => {
    expect(zoomTo(initialView(400, 300), 9).zoom).toBe(MAX_ZOOM);
    expect(zoomTo(initialView(400, 300), 0.5).zoom).toBe(1);
    const v = clampView({ width: 400, height: 300, zoom: 2, cx: 0, cy: 1000 });
    expect(v).toMatchObject({ cx: 75, cy: 225 });
    expect(cropRect(v)).toEqual({ x: 0, y: 150, width: 150, height: 150 });
  });

  it("a drag moves the square the other way, in picture pixels", () => {
    const v = zoomTo(initialView(400, 300), 2);
    // The frame's 240 px show 150 picture pixels: 48 px of drag is 30 of them.
    const moved = panBy(v, 48, -48, 240);
    expect(moved).toMatchObject({ cx: 170, cy: 180 });
    // Past the edge it stops there.
    expect(panBy(v, -10_000, 0, 240).cx).toBe(325);
  });

  it("zooming out again re-centres a square that would leave the picture", () => {
    const v = panBy(zoomTo(initialView(400, 300), 4), -10_000, 0, 240);
    expect(zoomTo(v, 1)).toMatchObject({ cx: 250, cy: 150 });
  });

  it("gives whole pixels, at least one, inside the picture", () => {
    const tiny = initialView(1, 1);
    expect(cropRect(zoomTo(tiny, 4))).toEqual({ x: 0, y: 0, width: 1, height: 1 });
    const r = cropRect({ width: 1001, height: 777, zoom: 3, cx: 1000, cy: 776 });
    expect(Number.isInteger(r.x) && Number.isInteger(r.width)).toBe(true);
    expect(r.x + r.width).toBeLessThanOrEqual(1001);
    expect(r.y + r.height).toBeLessThanOrEqual(777);
  });
});
