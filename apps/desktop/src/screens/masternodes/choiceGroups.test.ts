// The masternode sheets' choices are radio groups, not fieldsets with a legend (issue 108's WebKitGTK
// first-click shape): each `.mg-choices` group carries role="radiogroup" and is named by a visible
// element of the same sheet through aria-labelledby, which a screen reader announces as the legend was.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const SHEETS = ["RegisterSheet.tsx", "ChangeKeysSheet.tsx"];

const source = (file: string) => readFileSync(new URL(`./${file}`, import.meta.url), "utf8");

/** Every opening tag whose className is exactly `mg-choices`. */
const groups = (src: string) => [...src.matchAll(/<(\w+)\s+className="mg-choices"([^>]*)>/g)].map((m) => ({ tag: m[1], attrs: m[2] }));

/** The id expressions the file gives elements: `id="x"` or `id={`${id}-x`}`. */
const ids = (src: string) => new Set([...src.matchAll(/\bid=(?:"([^"]+)"|\{(`[^`]+`)\})/g)].map((m) => m[1] ?? m[2]));

const labelledBy = (attrs: string) => attrs.match(/aria-labelledby=(?:"([^"]+)"|\{(`[^`]+`)\})/)?.slice(1).find(Boolean);

describe("masternode choice groups", () => {
  for (const file of SHEETS)
    it(`${file}: no fieldset or legend, and every group is a radiogroup named by a visible element`, () => {
      const src = source(file);
      expect(src).not.toMatch(/<fieldset|<legend/);
      const found = groups(src);
      expect(found.length).toBeGreaterThan(0);
      const given = ids(src);
      for (const g of found) {
        expect(g.tag).toBe("div");
        expect(g.attrs).toContain('role="radiogroup"');
        const name = labelledBy(g.attrs);
        expect(name, g.attrs).toBeDefined();
        expect(given.has(name!), `${name} names no element in ${file}`).toBe(true);
      }
    });

  it("counts the wizard's five groups and Change keys' three", () => {
    expect(groups(source("RegisterSheet.tsx"))).toHaveLength(5);
    expect(groups(source("ChangeKeysSheet.tsx"))).toHaveLength(3);
  });
});
