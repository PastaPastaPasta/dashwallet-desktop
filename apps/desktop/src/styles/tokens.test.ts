import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const src = resolve(__dirname, "..");
const files = (dir: string): string[] =>
  readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    return statSync(p).isDirectory() ? (f === "generated" ? [] : files(p)) : [p];
  });

describe("tokens only (UX-SPEC §2, §7 global checklist)", () => {
  it("no colour literal outside the generated token files", () => {
    const offenders = files(src)
      .filter((p) => /\.(css|tsx?)$/.test(p) && !p.includes("tokens.generated") && !p.endsWith(".test.ts") && !p.endsWith("styleChecks.ts"))
      .flatMap((p) =>
        readFileSync(p, "utf8")
          .split("\n")
          .map((line, i) => ({ p, i: i + 1, line }))
          .filter(({ line }) => /#[0-9a-fA-F]{6}\b|#[0-9a-fA-F]{3}\b|\brgba?\(|\bhsla?\(/.test(line)),
      );
    expect(offenders.map((o) => `${o.p}:${o.i}: ${o.line.trim()}`)).toEqual([]);
  });

  it("every --dw-* property the stylesheet reads is generated", () => {
    const generated = readFileSync(join(src, "styles/tokens.generated.css"), "utf8");
    const defined = new Set([...generated.matchAll(/(--dw-[\w-]+):/g)].map((m) => m[1]));
    const used = files(src)
      .filter((p) => /\.(css|tsx)$/.test(p) && !p.includes("tokens.generated"))
      .flatMap((p) => [...readFileSync(p, "utf8").matchAll(/var\((--dw-[\w-]+)/g)].map((m) => m[1]));
    expect([...new Set(used)].filter((u) => !defined.has(u))).toEqual([]);
  });
});
