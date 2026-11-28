import type { Locator } from "@playwright/test";

/** Compare text or a masked fill with the background behind it. */
export async function contrast(el: Locator, paint: "text" | "mask" = "text"): Promise<number> {
  return el.evaluate((node, paint) => {
    const parse = (c: string): [number, number, number, number] => {
      const m = c.match(/-?[\d.]+/g)!.map(Number);
      if (c.startsWith("color(")) return [m[0] * 255, m[1] * 255, m[2] * 255, m[3] ?? 1];
      return [m[0], m[1], m[2], m[3] ?? 1];
    };
    const over = (top: number[], bottom: number[]) => {
      const a = top[3];
      return [0, 1, 2].map((i) => top[i] * a + bottom[i] * (1 - a)).concat(1);
    };
    const layers: number[][] = [];
    for (let n: Element | null = paint === "mask" ? node.parentElement : node; n; n = n.parentElement) {
      const bg = parse(getComputedStyle(n).backgroundColor);
      if (bg[3] > 0) layers.push(bg);
      if (bg[3] === 1) break;
    }
    let base: number[] = [255, 255, 255, 1];
    for (const l of layers.reverse()) base = over(l, base);
    const style = getComputedStyle(node);
    const fg = over(parse(paint === "mask" ? style.backgroundColor : style.color), base);
    const lum = (c: number[]) => {
      const [r, g, b] = c.slice(0, 3).map((v) => { const s = v / 255; return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4; });
      return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };
    const [hi, lo] = [lum(fg), lum(base)].sort((a, b) => b - a);
    return (hi + 0.05) / (lo + 0.05);
  }, paint);
}
