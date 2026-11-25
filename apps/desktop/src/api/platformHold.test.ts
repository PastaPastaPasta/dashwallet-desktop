// C6/C7: every Platform or Shielded funds flow says the restore's hold and Dash Platform being off for
// the session in MG's words, and keeps its own line for Platform that can't be reached or is off on
// this network.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { claimErrorText } from "../dashpay/claim";
import { startErrorText } from "../dashpay/registration";
import { topUpErrorText } from "../dashpay/topup";
import { fromPlatformErrorText } from "../dashpay/topupSource";
import { withdrawErrorText } from "../dashpay/withdraw";
import { fundErrorText } from "../platform/fund";
import { moveErrorText } from "../platform/move";
import { sendErrorText } from "../platform/send";
import { fromErrorText } from "../screens/pay/fromBalance";
import { shieldedErrorText } from "../shielded/flow";
import { platformHoldText } from "./platformHold";

const RESTORED =
  "Dash Wallet hasn't finished checking this wallet with Dash Platform since it restored your wallet data from before the upgrade, so Platform and Shielded funds are paused and nothing was sent. The check runs while Dash Platform is on.";
const OFF = "Nothing was sent: Dash Platform is off until you next start Dash Wallet, and this needs it.";
const OFF_NOT_SENDING = "Dash Platform is off until you next start Dash Wallet, and this needs it.";

const restored = { code: "platform.restored", message: "restored" };
const OFF_FORMS = ["platform.feature_off", "platform.unavailable"].map((code) => ({ code, message: "platform off", data: { kind: "feature", feature: "platform" } }));
const offOnNetwork = { code: "platform.feature_off", message: "off on mainnet", data: { kind: "feature", feature: "platform_registration" } };
const unreachable = { code: "platform.unavailable", message: "DAPI did not answer" };

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
  for (const [flow, text] of Object.entries(FLOWS))
    it(`${flow}: platform.restored and Platform off for the session each say MG's line; unreachable and off on this network keep their own`, () => {
      expect(text(restored)).toBe(RESTORED);
      for (const off of OFF_FORMS) expect(text(off), off.code).toBe(OFF);
      for (const other of [unreachable, offOnNetwork]) {
        expect(text(other), other.code).not.toBe(OFF);
        expect(text(other), other.code).not.toBe(RESTORED);
      }
    });

  it("says Platform off without \"Nothing was sent\" where nothing was being sent", () => {
    for (const off of OFF_FORMS) {
      expect(platformHoldText(off), off.code).toBe(OFF);
      expect(platformHoldText(off, false), off.code).toBe(OFF_NOT_SENDING);
    }
    expect(platformHoldText(restored, false)).toBe(RESTORED);
    expect(platformHoldText(unreachable, false)).toBeNull();
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
    expect(platformHoldText(unreachable)).toBeNull();
    expect(platformHoldText(offOnNetwork)).toBeNull();
    expect(platformHoldText({ code: "platform.unavailable", message: "x", data: { kind: "feature", feature: "platform_addresses" } })).toBeNull();
    expect(platformHoldText(new Error("platform.restored"))).toBeNull();
  });
});
