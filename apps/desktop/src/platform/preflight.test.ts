// #773: Platform's structure pre-check refusing a move to the Dash wallet, a send to a Platform address or
// a top-up from Platform, before anything is signed. Each refusal kind maps to its line, the consensus
// error in the message never shows, and the fixture records nothing (no Activity row, the grant unused).

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IpcBackend } from "../api/tauri";
import { fromPlatformErrorText } from "../dashpay/topupSource";
import { IDENTITY } from "../fixture/dashpay";
import { installFixture, type FixtureHandle } from "../fixture/index";
import { PLATFORM_PAYEE } from "../fixture/pending";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W } from "../fixture/scenarios";
import { t } from "../i18n";
import { moveErrorText } from "./move";
import { sendRefusalText } from "./send";

const RULES = "Dash Platform's rules have changed, and this version of Dash Wallet can't send this under them. Nothing was sent, and your balance is unchanged. Update Dash Wallet, then try again.";
const REFUSED = "Dash Platform would refuse this transfer as Dash Wallet built it, so nothing was sent and your balance is unchanged. If this keeps happening, update Dash Wallet.";
const RAW = "FeeStrategyTooManyStepsError: Fee strategy has 1 steps, maximum allowed is 0";
const KINDS = [
  { preCheck: "refused", code: "credits.preflight_refused", line: REFUSED },
  { preCheck: "rulesUnsupported", code: "credits.rules_unsupported", line: RULES },
] as const;

const credits = (c: string) => ({ kind: "credits" as const, credits: c });
const dash = (c: string) => `${c} credits`;
const duffs = (d: number) => `${d} duffs`;
const passphrase = { kind: "passphrase" as const, passphrase: FIXTURE_PASSPHRASE };

let fixture: FixtureHandle;
let backend: IpcBackend;
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  vi.stubGlobal("sessionStorage", { getItem: () => null, setItem: () => undefined, removeItem: () => undefined });
  fixture = installFixture({ scenario: "dashpay-registered" });
  backend = new IpcBackend("fixture");
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

const refusal = (p: Promise<unknown>) => p.then(() => null, (e: unknown) => e);

describe("a pre-check refusal's copy", () => {
  it("is the catalog's two lines, word for word", () => {
    expect(t("platform.preflight.rulesUnsupported")).toBe(RULES);
    expect(t("platform.preflight.refused")).toBe(REFUSED);
  });

  for (const k of KINDS) {
    const e = { code: k.code, message: RAW, data: null };

    it(`${k.code}: a move to the Dash wallet shows its line, never the consensus error`, () => {
      expect(moveErrorText(e, dash)).toBe(k.line);
    });

    it(`${k.code}: a send to a Platform address shows its line, never the consensus error`, () => {
      expect(sendRefusalText(e, dash)).toBe(k.line);
    });

    it(`${k.code}: a top-up from Platform shows its line, never the consensus error`, () => {
      expect(fromPlatformErrorText(e, duffs)).toBe(k.line);
      expect(fromPlatformErrorText(e, duffs, true)).toBe(k.line);
    });
  }
});

describe("a pre-check refusal on the fixture backend", () => {
  for (const k of KINDS) {
    it(`${k.code}: the move, the send and the top-up are refused with nothing recorded or redeemed`, async () => {
      const p = fixture.world.pending.platform!;
      const rows = p.activity.length;
      const balances = JSON.stringify(p.addresses);
      p.preCheck = k.preCheck;

      const move = await backend.platformMoveToCoreQuote(W, { kind: "all" });
      const moveGrant = await backend.authorize({ kind: "platformOp", maxDuffs: 0, maxCredits: move.grant.maxCredits }, W, passphrase);
      const moved = await refusal(backend.platformMoveToCore(W, { kind: "all" }, move.to, move.exitOf, moveGrant.id));
      expect(moved).toMatchObject({ code: k.code, data: null });
      expect(moveErrorText(moved, dash)).toBe(k.line);

      const send = await backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), null);
      const sendGrant = await backend.authorizePlatformSend(W, send.operation, passphrase);
      const sent = await refusal(backend.platformSend(W, send.operation, sendGrant.id));
      expect(sent).toMatchObject({ code: k.code, data: null });
      expect(sendRefusalText(sent, dash)).toBe(k.line);

      const top = await backend.creditsTopUpFromPlatformQuote(W, IDENTITY.alice, credits("1000000000"));
      const topGrant = await backend.authorize({ kind: "platformOp", maxDuffs: 0, maxCredits: top.grant.maxCredits }, W, passphrase);
      const topped = await refusal(backend.creditsTopUpFromPlatform(W, IDENTITY.alice, credits("1000000000"), null, topGrant.id));
      expect(topped).toMatchObject({ code: k.code, data: null });
      expect(fromPlatformErrorText(topped, duffs)).toBe(k.line);

      expect(p.activity).toHaveLength(rows);
      expect(JSON.stringify(p.addresses)).toBe(balances);
      // The grants were not redeemed: once the rules agree, the same grants pay.
      delete p.preCheck;
      expect(await backend.platformMoveToCore(W, { kind: "all" }, move.to, move.exitOf, moveGrant.id)).toMatchObject({ state: "done" });
      expect(p.activity).toHaveLength(rows + 1);
    });
  }
});
