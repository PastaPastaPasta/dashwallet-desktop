import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import type { PendingPlatformFund } from "../api/generated/PendingPlatformFund";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import { resetShielded } from "../shielded/flow";
import { resetPlatformMoves } from "../platform/move";
import { resetPlatformFunds } from "../platform/fund";
import { ShieldedCard } from "./ShieldedSheet";
import { PlatformMoveCard } from "./PlatformMoveSheet";

const reads = vi.hoisted(() => ({ shielded: null as ShieldedStatus | null, funds: [] as PendingPlatformFund[], activity: [] as PlatformActivity[] }));
vi.mock("../api/queries", async (original) => ({
  ...await original<typeof import("../api/queries")>(),
  useBackend: () => ({}),
  useShieldedStatus: () => ({ data: reads.shielded }),
  usePendingPlatformFunds: () => ({ data: reads.funds }),
  usePlatformActivity: () => ({ data: reads.activity, isPending: false, isError: false }),
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
const shielded = () => renderToStaticMarkup(createElement(ShieldedCard, { walletId: "fixture", network }));
const platform = (dustCredits = "0") => renderToStaticMarkup(createElement(PlatformMoveCard, { walletId: "fixture", network, platform: 40_000_000, dustCredits }));

beforeEach(() => {
  resetShielded();
  resetPlatformMoves();
  resetPlatformFunds();
  reads.shielded = { active: true, protocolVersion: 13, activationVersion: 12, ready: true, balance: "125000000000", pending: "0", pendingShields: [], unresolvedSpends: [], unrecordedSpends: [], syncedIndex: 4096, lastSyncAt: 1, syncing: false, proverReady: true };
  reads.funds = [];
  reads.activity = [];
});

describe("Home pool attention sections", () => {
  it("renders no Shielded section in the normal state", () => {
    expect(shielded()).toBe("");
  });

  it("renders no Platform section in the normal state", () => {
    expect(platform()).toBe("");
  });

  it("keeps a pending shield and its Finish, without description or Transfer", () => {
    reads.shielded!.pendingShields = [{ txid: "pending", duffs: 50_000_000, proved: false, consumptionUnknown: false }];
    reads.shielded!.pending = "25000000000";
    const html = shielded();
    expect(html).toContain('data-testid="shielded-card-pending"');
    expect(html).toContain('data-testid="shielded-unfinished"');
    expect(html).toContain('data-testid="shielded-finish"');
    expect(html).not.toContain('data-testid="shielded-card-text"');
    expect(html).not.toContain('data-testid="shielded-card-transfer"');
  });

  it("keeps Check again and its syncing reason", () => {
    reads.shielded!.ready = false;
    reads.shielded!.syncing = true;
    reads.shielded!.pendingShields = [{ txid: "checking", duffs: 50_000_000, proved: true, consumptionUnknown: true }];
    const html = shielded();
    expect(html).toContain('data-testid="shielded-checking"');
    expect(html).toContain("Check again");
    expect(html).toContain('data-testid="shielded-card-why"');
    expect(html).toContain("disabled");
    expect(html).not.toContain('data-testid="shielded-card-text"');
  });

  it("keeps unrecorded alerts and unresolved spend warnings", () => {
    reads.shielded!.unrecordedSpends = [{ activityId: "lost", since: 1 }];
    reads.shielded!.unresolvedSpends = [{ activityId: "unresolved", transition: "00", attempts: 0 }];
    const html = shielded();
    expect(html).toContain('role="alert" data-testid="shielded-unrecorded"');
    expect(html).toContain('data-testid="shielded-unresolved"');
    expect(html).not.toContain('data-testid="shielded-card-text"');
  });

  it.each([false, true])("keeps unfinished Platform funds and Finish or Check again (%s)", (consumptionUnknown) => {
    reads.funds = [{ txid: "fund", duffs: 50_000_000, proved: true, consumptionUnknown, state: consumptionUnknown ? "checking" : "ready" }];
    const html = platform();
    expect(html).toContain(`data-testid="platform-fund-${consumptionUnknown ? "checking" : "unfinished"}"`);
    expect(html).toContain('data-testid="platform-fund-finish"');
    expect(html).not.toContain('data-testid="platform-card-amount"');
  });

  it("keeps dust without a repeated balance", () => {
    const html = platform("1500000");
    expect(html).toContain('data-testid="platform-card-dust"');
    expect(html).not.toContain('data-testid="platform-card-amount"');
  });
});
