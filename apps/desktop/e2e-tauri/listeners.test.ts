import { describe, expect, it } from "vitest";
import { ownedListeners } from "./listeners.ts";

describe("the fixture writer's drivers are owned before the first request (#101)", () => {
  it("waits while a port has no listener, and accepts only both ports owned in one snapshot", () => {
    expect(ownedListeners([{ port: 1, groups: [] }, { port: 2, groups: [] }], 7)).toBeUndefined();
    expect(ownedListeners([{ port: 1, groups: [7] }, { port: 2, groups: [] }], 7)).toBeUndefined();
    expect(ownedListeners([{ port: 1, groups: [7] }, { port: 2, groups: [7, 7] }], 7)).toBe(true);
  });

  it("refuses a foreign or invisible listener even while the other port is still empty", () => {
    expect(() => ownedListeners([{ port: 1, groups: [8] }, { port: 2, groups: [] }], 7)).toThrow(/port 1 .*want 7/);
    expect(() => ownedListeners([{ port: 1, groups: [7] }, { port: 2, groups: [7, null] }], 7)).toThrow(/port 2/);
    expect(() => ownedListeners([{ port: 1, groups: [7] }, { port: 2, groups: [7] }], undefined)).toThrow();
  });
});
