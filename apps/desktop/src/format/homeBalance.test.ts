import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Balances } from "../api/generated/Balances";
import type { ListArrival } from "../components/emptyStates";
import { compact, masked } from "./amount";
import {
  breakdownRows,
  coreSubtitle,
  HERO_FIRST_READ_QUIET_MS,
  HERO_NEW_BLOCK_HOLD_MS,
  heroDigits,
  heroQuietEnds,
  HERO_SYNC,
  heroSync,
  heroSyncLine,
  homeBalance,
  newHeroWatch,
  POOL_ROWS,
  shieldedSubtitle,
  watchHero,
  type HeroReading,
  type HeroSync,
  type HeroWatch,
  type PoolReading,
} from "./homeBalance";

const COIN = 100_000_000;
const core: Balances = { confirmed: 3 * COIN, unconfirmed: COIN, immature: 0, locked: 0, total: 4 * COIN, coinjoin: 2 * COIN, withheld: 0, catchingUp: false, heldUnfinished: 0, heldUnvalued: 0, heldAwaiting: false };
const fmt = (duffs: number) => (duffs / COIN).toFixed(2);

describe("Home's balance", () => {
  it("keeps Core pending out of both the hero total and the Core amount", () => {
    const withPending = { ...core, withheld: COIN / 2 };
    const b = homeBalance(withPending, 5 * COIN, COIN / 2);
    expect(b.pools.core).toBe(3 * COIN);
    expect(b.total).toBe(8.5 * COIN);
    expect(coreSubtitle(withPending, fmt, "Dash wallet")).toBe("1.50 pending");
  });

  it("is Core (mixed coins included) plus Shielded plus Platform", () => {
    const b = homeBalance(core, 5 * COIN, COIN / 2);
    expect(b.total).toBe(8.5 * COIN);
    expect(b.pools).toEqual({ core: 3 * COIN, shielded: 5 * COIN, platform: COIN / 2 });
  });

  it("counts only known pools in the hero, and keeps why the others have no number", () => {
    for (const [shielded, platform] of [["loading", "failed"], ["absent", "loading"], ["failed", "absent"]] as const) {
      const b = homeBalance(core, shielded, platform);
      expect(b.total).toBe(3 * COIN);
      expect(b.pools).toEqual({ core: 3 * COIN, shielded, platform });
    }
    expect(homeBalance(core, 0, 0).pools).toMatchObject({ shielded: 0, platform: 0 });
    expect(homeBalance(core, 5 * COIN, "failed").total).toBe(8 * COIN);
  });

  it("counts only what can be spent: immature and locked funds stay out of the hero and the Core row", () => {
    const b = homeBalance({ confirmed: COIN, unconfirmed: 0, immature: 2 * COIN, locked: 3 * COIN, total: 6 * COIN, coinjoin: 0, withheld: 0, catchingUp: false, heldUnfinished: 0, heldUnvalued: 0, heldAwaiting: false }, 0, 0);
    expect(b.total).toBe(COIN);
    expect(b.pools.core).toBe(COIN);
  });

  it("names what is pending, immature and locked under the Core row", () => {
    expect(coreSubtitle(core, fmt, "Dash wallet")).toBe("1.00 pending");
    const held = { ...core, unconfirmed: 0, immature: 2 * COIN, locked: 10 * COIN };
    expect(coreSubtitle(held, fmt, "Dash wallet")).toBe("2.00 immature · 10.00 locked");
    expect(coreSubtitle({ ...held, immature: 0, locked: 0 }, fmt, "Dash wallet")).toBe("Dash wallet");
  });

  it("names under the Shielded row what payments that may still go through hold, and how many", () => {
    expect(shieldedSubtitle({ count: 1, credits: "30000000000", duffs: 3 * COIN / 10 }, fmt, "Private balance")).toBe("0.30 held for a payment");
    expect(shieldedSubtitle({ count: 2, credits: "60000000000", duffs: 6 * COIN / 10 }, fmt, "Private balance")).toBe("0.60 held for 2 payments");
    expect(shieldedSubtitle(null, fmt, "Private balance")).toBe("Private balance");
  });

  it("names Shielded's pending credits on the Shielded row without changing its spendable amount", () => {
    const b = homeBalance(core, 0, COIN / 2);
    expect(shieldedSubtitle(null, fmt, "Private balance", "50000000000")).toBe("0.50 pending");
    expect(shieldedSubtitle({ count: 1, credits: "30000000000", duffs: 3 * COIN / 10 }, fmt, "Private balance", "50000000000")).toBe("0.50 pending · 0.30 held for a payment");
    expect(shieldedSubtitle(null, fmt, "Private balance", "1")).toBe("1 credit pending");
    expect(shieldedSubtitle(null, fmt, "Private balance", "1001")).toBe("1,001 credits pending");
    expect(b.pools.shielded).toBe(0);
    expect(POOL_ROWS.find((row) => row.pool === "platform")?.subtitle).toBe("Platform addresses");
  });

  it("always lists Core, Shielded and Platform, in that order", () => {
    expect(POOL_ROWS.map((r) => r.pool)).toEqual(["core", "shielded", "platform"]);
  });
});

describe("Home's breakdown rows", () => {
  const rows = (shielded: PoolReading, platform: PoolReading) => breakdownRows(homeBalance(core, shielded, platform).pools).map((row) => row.pool);

  it("reserves the same cells while a supported pool has zero spendable funds", () => {
    expect(rows(0, 0)).toEqual(rows(COIN, COIN));
    expect(rows("absent", "absent")).toEqual(["core"]);
  });

  it("omits a pool only when the wallet has none", () => {
    expect(rows(5 * COIN, COIN / 2)).toEqual(["core", "shielded", "platform"]);
    expect(rows(0, COIN / 2)).toEqual(["core", "shielded", "platform"]);
    expect(rows("absent", COIN / 2)).toEqual(["core", "platform"]);
    expect(rows(1, 0)).toEqual(["core", "shielded", "platform"]);
  });

  it("keeps the row of a pool still being read or whose read failed", () => {
    expect(rows("loading", 0)).toEqual(["core", "shielded", "platform"]);
    expect(rows(0, "failed")).toEqual(["core", "shielded", "platform"]);
    expect(rows("loading", "failed")).toEqual(["core", "shielded", "platform"]);
    expect(rows("absent", "loading")).toEqual(["core", "platform"]);
  });

});

describe("the hero's digits", () => {
  it("keeps two decimals at full size and the rest smaller, without rounding", () => {
    expect(heroDigits(compact(3_204_698_287, false))).toEqual({ major: "32.04", minor: "698287" });
    expect(heroDigits(compact(125_000_000, false))).toEqual({ major: "1.25", minor: "" });
    expect(heroDigits(compact(100_000_001, false))).toEqual({ major: "1.00", minor: "000001" });
    expect(heroDigits(compact(0, false))).toEqual({ major: "0.00", minor: "" });
  });

  it("joins back into the exact amount in every unit, and leaves discreet and duffs alone", () => {
    for (const unit of ["DASH", "mDASH", "uDASH", "duffs"] as const) {
      const text = compact(123_456_789_012, false, unit);
      const { major, minor } = heroDigits(text);
      expect(major + minor).toBe(text);
    }
    expect(heroDigits(compact(123_456_789_012, false, "duffs")).minor).toBe("");
    expect(heroDigits(masked())).toEqual({ major: masked(), minor: "" });
  });
});

describe("the line above Home's balance", () => {
  const line = (b: Balances | null, arrival: ListArrival, running: boolean, more: Partial<HeroSync> = {}) =>
    heroSyncLine(b, { arrival, running, readFailed: false, atTip: arrival === "settled", quiet: false, ...more });
  const zero = { ...core, confirmed: 0, unconfirmed: 0, total: 0, coinjoin: 0 };
  const catchingUp = { ...core, catchingUp: true };

  it("is empty only when this wallet is scanned to the tip", () => {
    expect(line(core, "settled", true)).toBe("");
    expect(line(zero, "settled", false)).toBe("");
    expect(line(null, "settled", true)).toBe("");
  });

  it("says the balance may change while this wallet syncs, the network at the tip or not", () => {
    for (const b of [core, zero, catchingUp]) expect(line(b, "syncing", true)).toBe(HERO_SYNC.syncing);
    expect(HERO_SYNC.syncing).toBe("Still syncing, balance may change");
    expect(line(core, "starting", true)).toBe(HERO_SYNC.syncing);
  });

  it("says syncing stopped when the wallet is behind and sync is not running", () => {
    for (const b of [core, zero]) {
      expect(line(b, "stopped", false)).toBe("Syncing has stopped, balance may be out of date");
      expect(line(b, "syncing", false)).toBe(HERO_SYNC.stopped);
    }
  });

  it("never calls a birth-block balance final, even if the scan reads settled", () => {
    expect(line(catchingUp, "settled", true)).toBe(HERO_SYNC.syncing);
    expect(line(catchingUp, "settled", false)).toBe(HERO_SYNC.stopped);
  });

  it("names the starting and unreadable cases without claiming a sync", () => {
    expect(line(core, "starting", false)).toBe("Not synced yet, balance may change");
    expect(line(core, "unreadable", true)).toBe("Could not check syncing, balance may change");
  });

  it("keeps an unknown balance's caption, and does not say Syncing when sync is not running", () => {
    expect(line(null, "syncing", true)).toBe("Syncing Balance");
    expect(line(null, "stopped", false)).toBe("Not synced yet");
    expect(line(null, "starting", false)).toBe("Not synced yet");
  });

  it("needs this wallet's own checkpoint at the tip, not only a settled reading", () => {
    expect(line(core, "settled", true, { atTip: false })).toBe(HERO_SYNC.syncing);
    expect(line(core, "settled", false, { atTip: false })).toBe(HERO_SYNC.stopped);
  });

  it("says it could not check after a failed read, over a settled reading and the quiet", () => {
    expect(line(core, "settled", true, { readFailed: true })).toBe(HERO_SYNC.unreadable);
    expect(line(core, "settled", true, { readFailed: true, quiet: true })).toBe(HERO_SYNC.unreadable);
    expect(line(zero, "settled", false, { readFailed: true })).toBe(HERO_SYNC.unreadable);
    expect(line(null, "settled", true, { readFailed: true })).toBe("Syncing Balance");
  });

  describe("over time", () => {
    const TIP = 1000;
    const at = (more: Partial<HeroReading> = {}): HeroReading => ({
      walletId: "w1",
      arrival: "settled",
      running: true,
      networkCaughtUp: true,
      tip: TIP,
      scanHeight: TIP,
      scanPending: false,
      readFailed: false,
      ...more,
    });
    const behind = (tip: number, scanHeight: number, more: Partial<HeroReading> = {}) => at({ arrival: "syncing", networkCaughtUp: false, tip, scanHeight, ...more });
    const pending = (more: Partial<HeroReading> = {}) => at({ arrival: "starting", scanHeight: undefined, scanPending: true, ...more });

    let watch: HeroWatch | null;
    const shown = (r: HeroReading, b: Balances | null = core) => {
      const now = Date.now();
      watch = watchHero(watch ?? newHeroWatch(r.walletId, now), r, now);
      return heroSyncLine(b, heroSync(watch, r, now));
    };
    const wait = (ms: number) => vi.advanceTimersByTime(ms);

    beforeEach(() => {
      vi.useFakeTimers();
      vi.setSystemTime(new Date("2026-11-20T12:00:00Z"));
      watch = null;
    });
    afterEach(() => vi.useRealTimers());

    describe("the first scan-height read", () => {
      it("is empty for 2 s from mount while the read is out, then says the balance may change, then empty at the tip", () => {
        expect(HERO_FIRST_READ_QUIET_MS).toBe(2_000);
        expect(shown(pending())).toBe("");
        wait(1_900);
        expect(shown(pending())).toBe("");
        wait(200);
        expect(shown(pending())).toBe(HERO_SYNC.syncing);
        expect(shown(at())).toBe("");
      });

      it("keeps an unknown balance's caption empty for the same 2 s", () => {
        expect(shown(pending(), null)).toBe("");
        wait(2_100);
        expect(shown(pending(), null)).toBe("Syncing Balance");
        expect(shown(at(), null)).toBe("");
      });

      it("times the 2 s from mount, for a timer to read again", () => {
        const w = newHeroWatch("w1");
        expect(heroQuietEnds(w, pending())).toBe(Date.now() + HERO_FIRST_READ_QUIET_MS);
        expect(heroQuietEnds(w, at())).toBeNull();
      });

      it("keeps the caveat from mount while the balance is catching up, the network is not caught up, or sync is not running", () => {
        expect(shown(pending(), catchingUp)).toBe(HERO_SYNC.syncing);
        expect(shown(pending({ networkCaughtUp: false, arrival: "syncing" }))).toBe(HERO_SYNC.syncing);
        expect(shown(pending({ running: false }))).toBe(HERO_SYNC.notStarted);
      });

      it("says it could not check once the read has failed", () => {
        expect(shown(pending({ scanPending: false, arrival: "unreadable", readFailed: true }))).toBe(HERO_SYNC.unreadable);
      });
    });

    describe("a failed read", () => {
      it("after a read at the tip, a rejected refetch says it could not check while the old value stays cached", () => {
        expect(shown(at())).toBe("");
        expect(shown(at({ readFailed: true }))).toBe(HERO_SYNC.unreadable);
        expect(shown(at())).toBe("");
      });

      it("wins over the first-read quiet and over a held new block", () => {
        expect(shown(pending({ readFailed: true }))).toBe(HERO_SYNC.unreadable);
        expect(shown(at())).toBe("");
        expect(shown(behind(TIP + 1, TIP))).toBe("");
        expect(shown(behind(TIP + 1, TIP, { readFailed: true }))).toBe(HERO_SYNC.unreadable);
        expect(shown(behind(TIP + 1, TIP))).toBe(HERO_SYNC.syncing);
      });
    });

    describe("a new block", () => {
      it("holds the line empty through one new block on a wallet seen at the tip", () => {
        expect(shown(at())).toBe("");
        expect(shown(behind(TIP + 1, TIP))).toBe("");
        wait(5_000);
        expect(shown(behind(TIP + 1, TIP))).toBe("");
        expect(shown(at({ tip: TIP + 1, scanHeight: TIP + 1 }))).toBe("");
        expect(shown(behind(TIP + 2, TIP + 1))).toBe("");
      });

      it("shows the caveat 30 s after the tip moved on a wallet stuck 2 behind", () => {
        expect(HERO_NEW_BLOCK_HOLD_MS).toBe(30_000);
        expect(shown(at())).toBe("");
        expect(shown(behind(TIP + 1, TIP))).toBe("");
        expect(heroQuietEnds(watch!, behind(TIP + 1, TIP))).toBe(Date.now() + HERO_NEW_BLOCK_HOLD_MS);
        wait(10_000);
        expect(shown(behind(TIP + 2, TIP))).toBe("");
        wait(19_900);
        expect(shown(behind(TIP + 2, TIP))).toBe("");
        wait(200);
        expect(shown(behind(TIP + 2, TIP))).toBe(HERO_SYNC.syncing);
        expect(shown(behind(TIP + 2, TIP + 1))).toBe(HERO_SYNC.syncing);
      });

      it("shows the caveat at once for a wallet first seen 2 behind and never seen at the tip", () => {
        expect(shown(behind(TIP, TIP - 2))).toBe(HERO_SYNC.syncing);
        expect(shown(behind(TIP, TIP - 1))).toBe(HERO_SYNC.syncing);
        expect(shown(at())).toBe("");
      });

      it("keeps the caveat more than 2 blocks behind, and once sync stops", () => {
        shown(at());
        expect(shown(behind(TIP + 3, TIP))).toBe(HERO_SYNC.syncing);
        shown(at({ tip: TIP + 3, scanHeight: TIP + 3 }));
        expect(shown(behind(TIP + 4, TIP + 3, { running: false }))).toBe(HERO_SYNC.stopped);
      });

      it("ends at once when the scan rewinds above birth, or resets", () => {
        shown(at());
        expect(shown(behind(TIP + 1, TIP))).toBe("");
        expect(shown(behind(TIP + 1, TIP - 500))).toBe(HERO_SYNC.syncing);
        expect(shown(behind(TIP + 1, TIP))).toBe(HERO_SYNC.syncing);
        shown(at({ tip: TIP + 1, scanHeight: TIP + 1 }));
        wait(HERO_FIRST_READ_QUIET_MS);
        expect(shown(behind(TIP + 2, TIP + 1))).toBe("");
        expect(shown(pending({ tip: TIP + 2 }))).toBe(HERO_SYNC.syncing);
      });

      it("treats a checkpoint above the tip as not at the tip, and ends a hold on it", () => {
        expect(shown(at({ scanHeight: TIP + 5 }))).toBe(HERO_SYNC.syncing);
        shown(at());
        expect(shown(behind(TIP + 1, TIP))).toBe("");
        expect(shown(behind(TIP + 1, TIP + 3))).toBe(HERO_SYNC.syncing);
      });

      it("ends at once when the tip goes down", () => {
        shown(at());
        expect(shown(behind(TIP + 2, TIP))).toBe("");
        expect(shown(behind(TIP + 1, TIP))).toBe(HERO_SYNC.syncing);
      });

      it("shows the caveat for an unknown tip while raw sync says caught up", () => {
        expect(shown(at({ tip: null }))).toBe(HERO_SYNC.syncing);
        expect(shown(at({ tip: 0, scanHeight: 0 }))).toBe(HERO_SYNC.syncing);
        shown(at());
        expect(shown(behind(TIP + 1, TIP))).toBe("");
        expect(shown(at({ tip: null }))).toBe(HERO_SYNC.syncing);
        expect(shown(behind(TIP + 1, TIP))).toBe(HERO_SYNC.syncing);
      });

      it("starts over on another wallet", () => {
        shown(at());
        expect(shown(behind(TIP + 1, TIP, { walletId: "w2" }))).toBe(HERO_SYNC.syncing);
        expect(watch!.walletId).toBe("w2");
      });

      it("never hides a balance still catching up to its birth block", () => {
        shown(at());
        expect(shown(behind(TIP + 1, TIP), catchingUp)).toBe(HERO_SYNC.syncing);
      });
    });
  });
});
