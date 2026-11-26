import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { ShieldedCheckName } from "../../api/generated/ShieldedCheckName";
import type { ShieldedReadiness } from "../../api/generated/ShieldedReadiness";
import type { usePoolBalances } from "../../api/queries";
import type { PayWithInputs } from "../../dashpay/payWith";
import { READINESS_FAILED, type StatusInputs } from "../../dashpay/status";
import type { RegistrationWizard } from "../../dashpay/wizard";
import { GrantModel } from "../../tools/grant";
import { WizardSteps } from "./Registration";

type PoolBalances = ReturnType<typeof usePoolBalances>;

const state = vi.hoisted(() => ({
  core: { availableDuffs: 10e8, totalDuffs: 3_002_260 } as PayWithInputs["core"],
  pools: { platform: 5e8, shielded: 2e8, shieldedHold: null } as unknown as PoolBalances,
  readiness: null as ShieldedReadiness | null,
  platform: undefined as PayWithInputs["platform"],
}));

vi.mock("../../onboarding/model", async (original) => ({
  ...(await original<typeof import("../../onboarding/model")>()),
  useModel: (model: unknown) => model,
}));
vi.mock("../../api/queries", async (original) => ({
  ...(await original<typeof import("../../api/queries")>()),
  useBackend: () => ({ locale: "en-US" }),
  usePoolBalances: () => state.pools,
}));
vi.mock("../../dashpay/queries", async (original) => ({
  ...(await original<typeof import("../../dashpay/queries")>()),
  useUsernameQuote: () => state.core,
  useShieldedReadiness: () => state.readiness,
  usePlatformSource: () => state.platform,
}));

const CHECKS: ShieldedCheckName[] = ["poolActive", "featureOn", "denomination", "balance", "noUnsettledSpend", "synced", "rested", "poolSize"];
const readiness = (failed: ShieldedCheckName | null, readyAt: number | null = null): ShieldedReadiness => ({
  denomination: "3000000000",
  checks: CHECKS.map((check) => ({ check, ready: check !== failed })),
  readyAt,
  poolNotes: failed === "poolSize" ? 120 : null,
  poolRequired: 250,
  change: "7000000000",
});

const wiz = {
  step: "source",
  source: null,
  setSource: () => undefined,
  pickSource: () => undefined,
  close: () => undefined,
  withoutPrivacy: () => undefined,
  grants: new GrantModel({} as never),
} as unknown as RegistrationWizard;

const inputs = {
  amount: (duffs: number) => `${duffs / 1e8} DASH`,
  discreet: false,
  date: () => "",
} as unknown as StatusInputs;

function renderStep(r: ShieldedReadiness | null, shieldedDuffs: number | null = 2e8) {
  state.readiness = r;
  state.pools = { platform: 5e8, shielded: shieldedDuffs, shieldedHold: null } as unknown as PoolBalances;
  return renderToStaticMarkup(createElement(WizardSteps, { wiz, walletId: "w", inputs, network: "testnet" }));
}

describe("PrivacyStep heading and body", () => {
  it("uses Pay for your username and omits the privacy body when Shielded is not offered", () => {
    for (const r of [READINESS_FAILED, readiness("poolActive"), readiness("featureOn"), readiness("denomination")]) {
      const html = renderStep(r);
      expect(html).toContain('data-testid="registration-pay-with"');
      expect(html).toContain("Pay for your username");
      expect(html).not.toContain("Make your username private");
      expect(html).not.toContain('data-testid="privacy-body"');
      expect(html).not.toContain("we recommend paying for the username from your Shielded balance");
      expect(html).toContain('data-testid="pay-with-core"');
      expect(html).toContain("Funds will be traceable to your username");
      expect(html).not.toContain('data-testid="pay-with-shielded"');
      expect(html).not.toContain('data-testid="privacy-shield-first"');
    }
  });

  it("keeps the privacy title and body when Shielded is offered and can pay", () => {
    const html = renderStep(readiness(null));
    expect(html).toContain("Make your username private");
    expect(html).toContain('data-testid="privacy-body"');
    expect(html).toContain("To help keep your payment activity private, we recommend paying for the username from your Shielded balance");
    expect(html).toContain('data-testid="pay-with-shielded"');
    expect(html).toContain('data-testid="pay-with-core"');
  });

  it("keeps the privacy title and body when Shielded is offered with a tip card", () => {
    for (const [r, bal] of [
      [readiness("balance"), 0],
      [readiness("balance"), 1_000_000],
      [readiness("rested", 1_700_000_000), 2e8],
      [readiness("poolSize"), 2e8],
      [readiness("synced"), 2e8],
    ] as const) {
      const html = renderStep(r, bal);
      expect(html).toContain("Make your username private");
      expect(html).toContain('data-testid="privacy-body"');
      expect(html).toContain('data-testid="privacy-tip"');
    }
  });
});
