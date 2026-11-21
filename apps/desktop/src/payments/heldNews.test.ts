import { describe, expect, it } from "vitest";
import { HeldNews } from "./heldNews";

describe("payments held while locked", () => {
  it("keeps each txid once, and takes them as lookups of one wallet's run, oldest lookup first, each newest first", () => {
    const held = new HeldNews();
    held.hold("w", ["a", "b"]);
    held.hold("v", ["c"]);
    held.hold("w", ["a", "d"]);
    expect(held.take()).toEqual({
      runs: [
        { walletId: "w", txids: ["b"] },
        { walletId: "v", txids: ["c"] },
        { walletId: "w", txids: ["d", "a"] },
      ],
      overflowed: false,
    });
    expect(held.size).toBe(0);
    expect(held.take()).toEqual({ runs: [], overflowed: false });
  });

  it("cuts a long run into bounded lookups", () => {
    const held = new HeldNews(50, 100, 2);
    held.hold("w", ["1", "2", "3", "4", "5"]);
    expect(held.take().runs).toEqual([
      { walletId: "w", txids: ["2", "1"] },
      { walletId: "w", txids: ["4", "3"] },
      { walletId: "w", txids: ["5"] },
    ]);
  });

  it("keeps the newest past a wallet's cap and past the total, and says some were left out", () => {
    const held = new HeldNews(3, 5);
    held.hold("w", ["1", "2", "3", "4"]);
    expect(held.size).toBe(3);
    held.hold("v", ["a", "b", "c"]);
    const { runs, overflowed } = held.take();
    expect(overflowed).toBe(true);
    expect(runs).toEqual([
      { walletId: "w", txids: ["4", "3"] },
      { walletId: "v", txids: ["c", "b", "a"] },
    ]);
    held.hold("w", ["5"]);
    expect(held.take().overflowed).toBe(false);
  });

  it("puts older payments before the held ones, the newest still kept past the total", () => {
    const held = new HeldNews(50, 3);
    held.hold("w", ["new"]);
    held.holdOlder([
      { walletId: "w", txid: "old1" },
      { walletId: "w", txid: "old2" },
      { walletId: "w", txid: "old3" },
      { walletId: "w", txid: "new" },
    ]);
    expect(held.take()).toEqual({ runs: [{ walletId: "w", txids: ["new", "old3", "old2"] }], overflowed: true });
  });

  it("drops a removed wallet's, and all of them on a clear", () => {
    const held = new HeldNews();
    held.hold("w", ["a"]);
    held.hold("v", ["b"]);
    held.drop("v");
    expect(held.take().runs).toEqual([{ walletId: "w", txids: ["a"] }]);
    held.hold("w", ["c"]);
    held.clear();
    expect(held.size).toBe(0);
  });
});
