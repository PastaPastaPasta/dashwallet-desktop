import { describe, expect, it } from "vitest";
import { FixtureBackend, FIXTURE_WALLET_ID } from "./fixture";

const launch = FixtureBackend.browserLaunch();

describe("fixture backend", () => {
  it("is deterministic and newest first", async () => {
    const a = await new FixtureBackend(launch).historyPage(FIXTURE_WALLET_ID, { categories: [], text: null, sort: "newestFirst", cursor: null, limit: 500 });
    const b = await new FixtureBackend(launch).historyPage(FIXTURE_WALLET_ID, { categories: [], text: null, sort: "newestFirst", cursor: null, limit: 500 });
    expect(a).toEqual(b);
    const times = a.rows.map((r) => r.timestamp ?? 0);
    expect([...times].sort((x, y) => y - x)).toEqual(times);
    expect(a.rows[0].amount).toBe(20_000_000);
  });

  it("pages with a cursor and filters by category", async () => {
    const f = new FixtureBackend(launch, "large");
    const first = await f.historyPage(FIXTURE_WALLET_ID, { categories: [], text: null, sort: "newestFirst", cursor: null, limit: 200 });
    expect(first.rows).toHaveLength(200);
    expect(first.totalMatching).toBe(10_003);
    const second = await f.historyPage(FIXTURE_WALLET_ID, { categories: [], text: null, sort: "newestFirst", cursor: first.nextCursor, limit: 200 });
    expect(second.rows[0].txid).not.toBe(first.rows[0].txid);
    const sent = await f.historyPage(FIXTURE_WALLET_ID, { categories: ["sent"], text: null, sort: "newestFirst", cursor: null, limit: 50 });
    expect(sent.rows.every((r) => r.category === "sent")).toBe(true);
  });

  it("unlocks only with the fixture passphrase", async () => {
    const f = new FixtureBackend(launch, "default", { locked: true });
    await expect(f.unlock("nope")).rejects.toMatchObject({ code: "vault.wrong_passphrase" });
    expect((await f.unlock("demo")).state).toBe("unlocked");
  });
});
