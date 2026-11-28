// A pre-check refusal of the exact retry of a held send, or of the move out of a held top-up: that
// attempt sent nothing, but the earlier item may still land, so the line says so and points to
// Activity. Only once Activity shows that item failed for good do the first-attempt lines come back.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import { IpcBackend } from "../api/tauri";
import { IDENTITY } from "../fixture/dashpay";
import { installFixture } from "../fixture/index";
import { PLATFORM_PAYEE } from "../fixture/pending";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W } from "../fixture/scenarios";
import { moveErrorText, plannedWithdrawalCredits } from "./move";
import { sendRefusalText } from "./send";

const REFUSED = "Dash Platform would refuse this transfer as DashPay Desktop built it, so nothing was sent and your balance is unchanged. If this keeps happening, update DashPay Desktop.";
const RULES = "Dash Platform's rules have changed, and this version of DashPay Desktop can't send this under them. Nothing was sent, and your balance is unchanged. Update DashPay Desktop, then try again.";
const REFUSED_RETRY = "Dash Platform would refuse this attempt as DashPay Desktop built it, so this attempt sent nothing. Your earlier payment may still go through, so check Activity before you pay again. If this keeps happening, update DashPay Desktop.";
const RULES_RETRY = "Dash Platform's rules have changed, and this version of DashPay Desktop can't send this under them, so this attempt sent nothing. Your earlier payment may still go through. Update DashPay Desktop, and check Activity before you pay again.";
const REFUSED_TOP_UP = "Dash Platform would refuse this move as DashPay Desktop built it, so this attempt sent nothing. Your earlier top-up may still go through, so check Activity before you try again. If this keeps happening, update DashPay Desktop.";
const RULES_TOP_UP = "Dash Platform's rules have changed, and this version of DashPay Desktop can't make this move under them, so this attempt sent nothing. Your earlier top-up may still go through. Update DashPay Desktop, and check Activity before you try again.";
const RAW = "FeeStrategyTooManyStepsError: Fee strategy has 1 steps, maximum allowed is 0";

const KINDS = [
  { preCheck: "refused", code: "credits.preflight_refused", first: REFUSED, retry: REFUSED_RETRY, topUp: REFUSED_TOP_UP },
  { preCheck: "rulesUnsupported", code: "credits.rules_unsupported", first: RULES, retry: RULES_RETRY, topUp: RULES_TOP_UP },
] as const;

const dash = (c: string) => `${c} credits`;
const passphrase = { kind: "passphrase" as const, passphrase: FIXTURE_PASSPHRASE };
const row = (id: string, kind: PlatformActivity["kind"], state: PlatformActivity["state"], mayLand: boolean): PlatformActivity => ({
  id,
  kind,
  state,
  credits: "1000000000",
  address: null,
  to: null,
  expectedDuffs: null,
  feeCredits: null,
  at: 1,
  mayLand,
  estimated: false,
});
const refusal = (p: Promise<unknown>) => p.then(() => null, (e: unknown) => e);

beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  vi.stubGlobal("sessionStorage", { getItem: () => null, setItem: () => undefined, removeItem: () => undefined });
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("a pre-check refusal beside an earlier item that still holds", () => {
  for (const k of KINDS) {
    const e = { code: k.code, message: RAW, data: null };

    it(`${k.code}: the retry of a held send says this attempt sent nothing and the payment may still go through`, () => {
      expect(sendRefusalText(e, dash, [row("ps-1", "send", "failed", true)], "ps-1")).toBe(k.retry);
      expect(sendRefusalText(e, dash, [row("ps-1", "send", "pending", false)], "ps-1")).toBe(k.retry);
    });

    it(`${k.code}: the exit of a held top-up says this attempt sent nothing and the top-up may still go through`, () => {
      expect(moveErrorText(e, dash, [row("pt-1", "toIdentity", "failed", true)], "pt-1")).toBe(k.topUp);
    });

    it(`${k.code}: the line stays while the earlier item lands or Activity has not answered`, () => {
      for (const activity of [[row("ps-1", "send", "done", false)], [], null, undefined]) {
        expect(sendRefusalText(e, dash, activity, "ps-1")).toBe(k.retry);
        expect(moveErrorText(e, dash, activity?.map((a) => ({ ...a, kind: "toIdentity" as const })), "ps-1")).toBe(k.topUp);
      }
    });

    it(`${k.code}: once the earlier item failed for good, or none was named, the first-attempt line`, () => {
      expect(sendRefusalText(e, dash, [row("ps-1", "send", "failed", false)], "ps-1")).toBe(k.first);
      expect(moveErrorText(e, dash, [row("pt-1", "toIdentity", "failed", false)], "pt-1")).toBe(k.first);
      expect(sendRefusalText(e, dash, [row("ps-1", "send", "failed", true)], null)).toBe(k.first);
      expect(moveErrorText(e, dash, [row("pt-1", "toIdentity", "failed", true)], null)).toBe(k.first);
      expect(sendRefusalText(e, dash, [row("ps-1", "send", "failed", true)])).toBe(k.first);
      expect(moveErrorText(e, dash, [row("pt-1", "toIdentity", "failed", true)])).toBe(k.first);
    });
  }
});

describe("on the fixture backend", () => {
  const setUp = (scenario: "platform-send-failed-may-land" | "dashpay-registered") => {
    return { fixture: installFixture({ scenario }), backend: new IpcBackend("fixture") };
  };

  for (const k of KINDS) {
    it(`${k.code}: the refused retry of a held send leaves it holding, and its line says so`, async () => {
      const { fixture, backend } = setUp("platform-send-failed-may-land");
      const p = fixture.world.pending.platform!;
      const quote = await backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "credits", credits: "1000000000" }, "ps-held");
      expect(quote.retryOf).toBe("ps-held");
      const grant = await backend.authorizePlatformSend(W, quote.operation, passphrase);
      p.preCheck = k.preCheck;
      const sent = await refusal(backend.platformSend(W, quote.operation, grant.id));
      expect(sent).toMatchObject({ code: k.code, data: null });
      const activity = await backend.platformActivity(W);
      expect(activity?.find((a) => a.id === "ps-held")).toMatchObject({ kind: "send", mayLand: true });
      expect(sendRefusalText(sent, dash, activity, quote.retryOf)).toBe(k.retry);
    });

    it(`${k.code}: the refused exit of a held top-up leaves it holding, and its line says so`, async () => {
      const { fixture, backend } = setUp("dashpay-registered");
      const p = fixture.world.pending.platform!;
      p.nextTopUp = "held";
      const top = await backend.creditsTopUpFromPlatformQuote(W, IDENTITY.alice, { kind: "credits", credits: "1000000000" });
      const topGrant = await backend.authorize({ kind: "platformOp", maxDuffs: 0, maxCredits: top.grant.maxCredits }, W, passphrase);
      await backend.creditsTopUpFromPlatform(W, IDENTITY.alice, { kind: "credits", credits: "1000000000" }, null, topGrant.id);
      const held = p.activity.find((a) => a.kind === "toIdentity" && a.mayLand);
      expect(held).toBeDefined();

      const move = await backend.platformMoveToCoreQuote(W, { kind: "all" });
      expect(move.exitOf).toBe(held!.id);
      const grant = await backend.authorize({ kind: "platformOp", maxDuffs: 0, maxCredits: move.grant.maxCredits }, W, passphrase);
      p.preCheck = k.preCheck;
      const moved = await refusal(backend.platformMoveToCore(W, { kind: "credits", credits: plannedWithdrawalCredits(move) }, move.to, move.exitOf, grant.id));
      expect(moved).toMatchObject({ code: k.code, data: null });
      const activity = await backend.platformActivity(W);
      expect(activity?.find((a) => a.id === held!.id)).toMatchObject({ mayLand: true });
      expect(moveErrorText(moved, dash, activity, move.exitOf)).toBe(k.topUp);
    });
  }
});
