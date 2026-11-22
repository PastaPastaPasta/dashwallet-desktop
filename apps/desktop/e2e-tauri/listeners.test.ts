import { describe, expect, it } from "vitest";
import { ownedListeners, snapshotOf } from "./listeners.ts";

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

describe("one ss listing per poll (#101)", () => {
  const line = (port: number, pid?: number) =>
    `LISTEN 0      128        127.0.0.1:${port}      0.0.0.0:*${pid === undefined ? "" : `    users:(("drv",pid=${pid},fd=3))`}`;
  const groupOf = (pid: string) => (pid === "100" || pid === "101" ? 7 : 9);

  it("waits when each listing shows only one of the two ports", () => {
    expect(ownedListeners(snapshotOf([line(4774, 100)].join("\n"), [4774, 4775], groupOf), 7)).toBeUndefined();
    expect(ownedListeners(snapshotOf([line(4775, 101)].join("\n"), [4774, 4775], groupOf), 7)).toBeUndefined();
    expect(ownedListeners(snapshotOf([line(4774, 100), line(4775, 101)].join("\n"), [4774, 4775], groupOf), 7)).toBe(true);
  });

  it("reads an invisible owner as null and another group as foreign", () => {
    expect(snapshotOf(line(4774), [4774], groupOf)).toEqual([{ port: 4774, groups: [null] }]);
    expect(() => ownedListeners(snapshotOf([line(4774, 100), line(4775, 200)].join("\n"), [4774, 4775], groupOf), 7)).toThrow(/port 4775/);
    expect(snapshotOf(line(14774, 100), [4774], groupOf)).toEqual([{ port: 4774, groups: [] }]);
  });
});
