// C6/C7: every Platform or Shielded funds flow says the restore's hold and Dash Platform being off for
// the session in MG's words, and keeps its own line for Platform that can't be reached or is off on
// this network. The engine's C7 refusal is a bare platform.unavailable: only restore_status's
// platformOff tells it from unreachable.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { claimErrorText } from "../dashpay/claim";
import { startErrorText } from "../dashpay/registration";
import { topUpErrorText } from "../dashpay/topup";
import { fromPlatformErrorText } from "../dashpay/topupSource";
import { withdrawErrorText } from "../dashpay/withdraw";
import { fundErrorText } from "../platform/fund";
import { moveErrorText } from "../platform/move";
import { sendErrorText } from "../platform/send";
import { acceptFailedText } from "../screens/pay/contact";
import { fromErrorText } from "../screens/pay/fromBalance";
import { shieldedErrorText } from "../shielded/flow";
import type { RestoreStatus } from "./generated/RestoreStatus";
import { platformHoldText, readPlatformOffFrom } from "./platformHold";
import { cachedPlatformOff, keys } from "./queries";

const RESTORED =
  "Dash Wallet hasn't finished checking this wallet with Dash Platform since it restored your wallet data from before the upgrade, so Platform and Shielded funds are paused and nothing was sent. The check runs while Dash Platform is on.";
const OFF = "Nothing was sent: Dash Platform is off until you next start Dash Wallet, and this needs it.";
const OFF_NOT_SENDING = "Dash Platform is off until you next start Dash Wallet, and this needs it.";

const restored = { code: "platform.restored", message: "restored" };
const featureOff = { code: "platform.feature_off", message: "platform off", data: { kind: "feature", feature: "platform" } };
const unavailable = { code: "platform.unavailable", message: "Platform is off for this session" };
const offOnNetwork = { code: "platform.feature_off", message: "off on mainnet", data: { kind: "feature", feature: "platform_registration" } };

const held = (platformOff: boolean): RestoreStatus => ({ waiting: ["platformPass"], historyWallets: [], clearFailed: false, platformOnly: true, platformOff });

let client: QueryClient;
beforeEach(() => {
  client = new QueryClient();
  readPlatformOffFrom(() => cachedPlatformOff(client));
});
afterEach(() => readPlatformOffFrom(() => false));

/** The session as restore_status says it: without Dash Platform. */
const withoutPlatform = () => client.setQueryData(keys.restoreStatus, held(true));

const n = String;
const FLOWS: Record<string, (e: unknown) => string> = {
  shielded: (e) => shieldedErrorText(e, n, n),
  fund: (e) => fundErrorText(e, n, n),
  platformSend: (e) => sendErrorText(e, n),
  platformMove: (e) => moveErrorText(e, n),
  topUp: (e) => topUpErrorText(e, n),
  topUpFromPlatform: (e) => fromPlatformErrorText(e, n),
  registration: (e) => startErrorText(e, n),
  claim: (e) => claimErrorText(e, n),
  withdraw: (e) => withdrawErrorText(e, n),
  payFromBalance: (e) => fromErrorText(e, null, n),
};

describe("the restore's Platform and Shielded hold, as each flow says it", () => {
  for (const [flow, text] of Object.entries(FLOWS)) {
    it(`${flow}: with Platform on, platform.unavailable is unreachable; restored and feature_off "platform" say MG's lines`, () => {
      expect(text(restored)).toBe(RESTORED);
      expect(text(featureOff)).toBe(OFF);
      for (const other of [unavailable, offOnNetwork]) {
        expect(text(other), other.code).not.toBe(OFF);
        expect(text(other), other.code).not.toBe(RESTORED);
      }
    });

    it(`${flow}: in a session restore_status says runs without Dash Platform, platform.unavailable says Dash Platform is off`, () => {
      withoutPlatform();
      expect(text(unavailable)).toBe(OFF);
      expect(text(offOnNetwork)).not.toBe(OFF);
    });
  }

  it("accept and pay: Dash Platform off says C7's line; any other refusal keeps its own, with nothing paid", () => {
    expect(acceptFailedText(featureOff)).toBe(OFF);
    for (const other of [unavailable, restored, offOnNetwork]) expect(acceptFailedText(other), other.code).toMatch(/\S Nothing was paid\.$/);
    withoutPlatform();
    expect(acceptFailedText(unavailable)).toBe(OFF);
  });

  it("says Platform off without \"Nothing was sent\" where nothing was being sent", () => {
    expect(platformHoldText(featureOff)).toBe(OFF);
    expect(platformHoldText(featureOff, false)).toBe(OFF_NOT_SENDING);
    expect(platformHoldText(unavailable, false)).toBeNull();
    withoutPlatform();
    expect(platformHoldText(unavailable)).toBe(OFF);
    expect(platformHoldText(unavailable, false)).toBe(OFF_NOT_SENDING);
    expect(platformHoldText(restored, false)).toBe(RESTORED);
  });

  it("reads Platform off only from a cached restore_status that says platformOff", () => {
    for (const cached of [undefined, null, held(false)]) {
      if (cached === undefined) client.removeQueries({ queryKey: keys.restoreStatus });
      else client.setQueryData(keys.restoreStatus, cached);
      expect(platformHoldText(unavailable), String(cached?.platformOff ?? cached)).toBeNull();
    }
    withoutPlatform();
    expect(platformHoldText(unavailable)).toBe(OFF);
  });

  it("the app reads the cached restore_status, which the shell keeps read while the wallet is unlocked", () => {
    expect(readFileSync(resolve(import.meta.dirname, "../main.tsx"), "utf8")).toContain("readPlatformOffFrom(() => cachedPlatformOff(client));");
    expect(readFileSync(resolve(import.meta.dirname, "../app/App.tsx"), "utf8")).toContain("useRestoreStatus(!locked);");
  });

  // A quote, a Max, the Shielded receive address and the claim's check send nothing.
  it("every place where nothing is being sent asks for the line without \"Nothing was sent\"", () => {
    const src = resolve(import.meta.dirname, "..");
    const sites: [string, string][] = [
      ["screens/ShieldedSheet.tsx", "platformHoldText(line.state.error, false) ?? shieldedErrorText("],
      ["screens/ShieldedSheet.tsx", "platformHoldText(quote.error, false) ?? shieldedErrorText("],
      ["screens/ShieldedSheet.tsx", "platformHoldText(address.error, false) ?? shieldedErrorText("],
      ["screens/dashpay/WithdrawSheet.tsx", "platformHoldText(quote.error, false) ?? withdrawErrorText("],
      ["screens/PlatformMoveSheet.tsx", "platformHoldText(quote.error, false) ?? moveErrorText("],
      ["screens/pay/PayFromPlatform.tsx", "platformHoldText(quote.error, false) ?? sendErrorText("],
      ["screens/pay/PayFromBalance.tsx", "platformHoldText(quote.error, false) ?? fromErrorText("],
      ["screens/PlatformFundSheet.tsx", "platformHoldText(quote.error, false) ?? fundErrorText("],
      ["screens/TransferSheet.tsx", "platformHoldText(line.state.error, false) ??"],
      ["screens/TransferSheet.tsx", "platformHoldText(quote.error, false) ?? moveErrorText("],
      ["screens/dashpay/TopUpSheet.tsx", "platformHoldText(quote.error, false) ?? topUpErrorText("],
      ["screens/dashpay/TopUpSheet.tsx", "platformHoldText(quote.error, false) ?? fromPlatformErrorText("],
      ["dashpay/claim.ts", "platformHoldText(i.statusError, false) ?? startErrorText("],
    ];
    for (const [file, call] of sites) expect(readFileSync(resolve(src, file), "utf8"), `${file}: ${call}`).toContain(call);
    expect(readFileSync(resolve(src, "screens/PlatformMoveSheet.tsx"), "utf8").split("platformHoldText(quote.error, false) ?? moveErrorText(").length).toBe(3);
  });

  it("answers nothing for any other failure", () => {
    expect(platformHoldText(unavailable)).toBeNull();
    expect(platformHoldText(offOnNetwork)).toBeNull();
    expect(platformHoldText({ code: "platform.unavailable", message: "x", data: { kind: "feature", feature: "platform_addresses" } })).toBeNull();
    expect(platformHoldText(new Error("platform.restored"))).toBeNull();
  });
});
