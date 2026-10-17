// DP5-01 D11: the "Links you opened" model.

import { describe, expect, it } from "vitest";
import { offerReady, openedAgo, savedLinkRows } from "./savedLinks";

describe("saved links", () => {
  it("DP5-01 says when a link was opened", () => {
    const now = 1_800_000_000;
    expect(openedAgo(now - 5, now)).toBe("Opened just now");
    expect(openedAgo(now + 30, now)).toBe("Opened just now");
    expect(openedAgo(now - 60, now)).toBe("Opened 1 minute ago");
    expect(openedAgo(now - 45 * 60, now)).toBe("Opened 45 minutes ago");
    expect(openedAgo(now - 3600, now)).toBe("Opened 1 hour ago");
    expect(openedAgo(now - 23 * 3600, now)).toBe("Opened 23 hours ago");
    expect(openedAgo(now - 86_400, now)).toBe("Opened yesterday");
    expect(openedAgo(now - 3 * 86_400, now)).toBe("Opened 3 days ago");
  });

  it("DP5-01 lists the newest first, titled by kind", () => {
    const rows = savedLinkRows(
      [
        { id: "1".padStart(32, "0"), kind: "invitation", stashedAt: 100, expiresAt: 200 },
        { id: "2".padStart(32, "0"), kind: "payment", stashedAt: 300, expiresAt: 400 },
        { id: "3".padStart(32, "0"), kind: "contact", stashedAt: 300, expiresAt: 400 },
      ],
      400,
    );
    expect(rows.map((r) => [r.title, r.invitation])).toEqual([
      ["Payment request", false],
      ["Contact", false],
      ["DashPay invitation", true],
    ]);
    expect(rows[0].when).toBe("Opened 1 minute ago");
  });

  it("DP5-01 offers once a wallet shows, unlocked, with nothing in front", () => {
    const ok = { pending: true, hasWallet: true, locked: false, covered: false };
    expect(offerReady(ok)).toBe(true);
    expect(offerReady({ ...ok, pending: false })).toBe(false);
    expect(offerReady({ ...ok, hasWallet: false })).toBe(false);
    expect(offerReady({ ...ok, locked: true })).toBe(false);
    expect(offerReady({ ...ok, covered: true })).toBe(false);
  });
});
