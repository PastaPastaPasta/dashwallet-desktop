import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import type { PendingPlatformFund } from "../api/generated/PendingPlatformFund";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import { resetShielded, shieldedFor } from "../shielded/flow";
import { moveFor, resetPlatformMoves } from "../platform/move";
import { fundFor, resetPlatformFunds } from "../platform/fund";
import { resetRecovery } from "../recovery/model";
import { resetTransfers } from "../transfer/model";
import * as PlatformMoveSheet from "./PlatformMoveSheet";
import * as ShieldedSheet from "./ShieldedSheet";
import { UnfinishedTransfersCard } from "./UnfinishedTransfersCard";

const reads = vi.hoisted(() => ({
  shielded: null as ShieldedStatus | null,
  funds: [] as PendingPlatformFund[],
  activity: [] as PlatformActivity[],
  activityRead: { isPending: false, isError: false },
  platform: 40_000_000 as number | null,
  platformDust: "0",
}));
vi.mock("../api/queries", async (original) => ({
  ...await original<typeof import("../api/queries")>(),
  useBackend: () => ({}),
  useShieldedStatus: () => ({ data: reads.shielded }),
  usePendingPlatformFunds: () => ({ data: reads.funds }),
  usePlatformActivity: () => ({ data: reads.activity, ...reads.activityRead, refetch: () => undefined }),
  usePoolBalances: () => ({ shielded: 125_000_000, platform: reads.platform, platformDust: reads.platformDust }),
  useUnfinishedTransfers: () => ({ isSuccess: true, data: { locks: [], complete: true, unread: [] } }),
}));
vi.mock("../app/ui", async (original) => ({
  ...await original<typeof import("../app/ui")>(),
  useUi: (select: (state: unknown) => unknown) => select({ discreet: false, prefs: { unit: "dash", decimalDigits: 8 } }),
}));
vi.mock("../onboarding/model", async (original) => ({
  ...await original<typeof import("../onboarding/model")>(),
  useModel: (model: unknown) => model,
}));
const network: NetworkInfo = { kind: "testnet", dirName: "testnet", unitName: "tDASH" };
const home = () => renderToStaticMarkup(createElement(UnfinishedTransfersCard, { walletId: "fixture", network }));
const op = (o: Partial<PlatformActivity>): PlatformActivity => ({ id: "op", kind: "moveToCore", state: "pending", credits: "40000000000", address: null, to: "y", expectedDuffs: null, feeCredits: null, at: 1, mayLand: true, estimated: false, ...o });
/** Every Home pool section test id, and text that only an attention row said. */
const POOL_IDS = ["platform-card", "shielded-card", "platform-card-dust", "platform-card-waiting", "platform-card-send-held", "shielded-card-pending", "shielded-unresolved", "shielded-unrecorded", "shielded-card-why", "platform-move-view"];

beforeEach(() => {
  resetShielded();
  resetPlatformMoves();
  resetPlatformFunds();
  resetRecovery();
  resetTransfers();
  reads.shielded = { active: true, protocolVersion: 13, activationVersion: 12, ready: true, balance: "125000000000", pending: "0", pendingShields: [], unresolvedSpends: [], unrecordedSpends: [], syncedIndex: 4096, lastSyncAt: 1, syncing: false, proverReady: true };
  reads.funds = [];
  reads.activity = [];
  reads.activityRead = { isPending: false, isError: false };
  reads.platform = 40_000_000;
  reads.platformDust = "0";
});

function expectNoPoolSection(html: string) {
  for (const id of POOL_IDS) expect(html).not.toContain(`data-testid="${id}"`);
}

describe("Home has no Platform or Shielded pool section", () => {
  it("has no pool card components left to render", () => {
    expect(Object.keys(PlatformMoveSheet)).not.toContain("PlatformMoveCard");
    expect(Object.keys(ShieldedSheet)).not.toContain("ShieldedCard");
  });

  it.each<[string, () => void]>([
    ["the normal state", () => {}],
    ["Platform dust only, 0.004005 tDASH", () => {
      reads.platform = 0;
      reads.platformDust = "400500000";
    }],
    ["a move Platform has not confirmed", () => {
      reads.activity = [op({})];
    }],
    ["a payment from Platform that may still go through", () => {
      reads.activity = [op({ kind: "send", state: "failed" })];
    }],
    ["Activity still being read", () => {
      reads.activityRead = { isPending: true, isError: false };
    }],
    ["a move running", () => {
      moveFor({} as never, "fixture").running = true;
    }],
    ["a Shielded amount on its way", () => {
      reads.shielded!.pending = "25000000000";
    }],
    ["an unresolved Shielded payment", () => {
      reads.shielded!.unresolvedSpends = [{ activityId: "unresolved", transition: "00", attempts: 0 }];
    }],
    ["an unrecorded Shielded payment", () => {
      reads.shielded!.unrecordedSpends = [{ activityId: "lost", since: 1 }];
    }],
    ["an add to Platform this version can't finish", () => {
      reads.funds = [{ txid: "small", duffs: 5_000, proved: true, consumptionUnknown: false, state: "tooSmall" }];
    }],
  ])("renders nothing in %s", (_, set) => {
    set();
    const html = home();
    expect(html).toBe("");
  });
});

describe("Home's actionable recovery rows live in Unfinished transfers", () => {
  it("keeps a pending shield and its Finish, without the Shielded section", () => {
    reads.shielded!.pendingShields = [{ txid: "pending", duffs: 50_000_000, proved: false, consumptionUnknown: false }];
    reads.shielded!.pending = "25000000000";
    const html = home();
    expect(html).toContain('data-testid="unfinished-card"');
    expect(html).toContain('data-testid="shielded-unfinished"');
    expect(html).toContain('data-testid="shielded-finish"');
    expectNoPoolSection(html);
  });

  it("keeps Check again, disabled with its own reason while Shielded syncs", () => {
    reads.shielded!.ready = false;
    reads.shielded!.syncing = true;
    reads.shielded!.pendingShields = [{ txid: "checking", duffs: 50_000_000, proved: true, consumptionUnknown: true }];
    const html = home();
    expect(html).toContain('data-testid="shielded-checking"');
    expect(html).toContain("Check again");
    expect(html).toContain("disabled");
    expect(html).toContain('class="dash-button-why"');
    expectNoPoolSection(html);
  });

  it.each([false, true])("keeps an unfinished add to Platform and Finish or Check again (%s)", (consumptionUnknown) => {
    reads.funds = [{ txid: "fund", duffs: 50_000_000, proved: true, consumptionUnknown, state: consumptionUnknown ? "checking" : "ready" }];
    reads.platformDust = "1500000";
    const html = home();
    expect(html).toContain('data-testid="unfinished-card"');
    expect(html).toContain(`data-testid="platform-fund-${consumptionUnknown ? "checking" : "unfinished"}"`);
    expect(html).toContain('data-testid="platform-fund-finish"');
    expect(html).toContain(consumptionUnknown ? "Check again" : "Finish");
    expectNoPoolSection(html);
  });

  it.each([
    ["shielded", "shielded-unseen", "shielded-show"],
    ["fund", "platform-fund-unseen", "platform-fund-show"],
    ["move", "platform-move-unseen", "platform-move-show"],
  ] as const)("keeps an unseen %s result and its Show", (flow, row, show) => {
    const f = flow === "shielded" ? shieldedFor({} as never, "fixture") : flow === "fund" ? fundFor({} as never, "fixture") : moveFor({} as never, "fixture");
    f.step = "refused";
    const html = home();
    expect(html).toContain('data-testid="unfinished-card"');
    expect(html).toContain(`data-testid="${row}"`);
    expect(html).toContain(`data-testid="${show}"`);
    expectNoPoolSection(html);
  });

  it("keeps the failed Activity check and its Check again", () => {
    reads.activityRead = { isPending: false, isError: true };
    const html = home();
    expect(html).toContain('data-testid="unfinished-card"');
    expect(html).toContain('data-testid="platform-move-why"');
    expect(html).toContain("Could not check for an unfinished move.");
    expect(html).toContain('data-testid="platform-move-check-again"');
    expectNoPoolSection(html);
  });
});
