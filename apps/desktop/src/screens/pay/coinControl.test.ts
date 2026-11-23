// Pay's coin control (U-05): a fee target's title, its time and its block count in one sentence.

import { afterEach, describe, expect, it } from "vitest";
import { DEFAULT_PREFS, ui } from "../../app/ui";
import { targetTitle } from "./CoinControl";

afterEach(() => ui.set({ prefs: DEFAULT_PREFS }));

describe("a fee target's title", () => {
  it("reads as before in English, one block or many", () => {
    expect([targetTitle(1), targetTitle(6), targetTitle(1008)]).toEqual(["2.5 minutes (1 block)", "15 minutes (6 blocks)", "42 hours (1008 blocks)"]);
  });

  it("says the block count in the sentence's own words, not a word of its own", () => {
    ui.set({ prefs: { ...DEFAULT_PREFS, language: "en-XA" } });
    for (const n of [1, 6]) {
      const title = targetTitle(n);
      expect(title).toMatch(/^⟦.+⟧$/);
      expect(title.match(/⟦/g)).toHaveLength(2);
    }
  });
});
