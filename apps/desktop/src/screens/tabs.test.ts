import { describe, expect, it } from "vitest";
import { tabTarget } from "./Routes";

describe("tab keys", () => {
  it("move to the tab drawn on that side, left to right", () => {
    expect(tabTarget("ArrowRight", 0, 3, false)).toBe(1);
    expect(tabTarget("ArrowLeft", 0, 3, false)).toBe(2);
    expect(tabTarget("ArrowRight", 2, 3, false)).toBe(0);
  });

  it("move to the tab drawn on that side, right to left", () => {
    expect(tabTarget("ArrowRight", 1, 3, true)).toBe(0);
    expect(tabTarget("ArrowLeft", 0, 3, true)).toBe(1);
    expect(tabTarget("ArrowRight", 0, 3, true)).toBe(2);
  });

  it("go to the first and last with Home and End, and ignore other keys", () => {
    expect(tabTarget("Home", 2, 3, true)).toBe(0);
    expect(tabTarget("End", 0, 3, true)).toBe(2);
    expect(tabTarget("ArrowDown", 0, 3, false)).toBeNull();
  });
});
