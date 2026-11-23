import { describe, expect, it } from "vitest";
import type { AppError } from "../api/generated/AppError";
import { destinationNote, withdrawDestinationText, withdrawUnverifiedText, type WithdrawDestination } from "./withdraw";

const err = (code: AppError["code"]): AppError => ({ code, message: code, data: null });
const UNVERIFIED = "Dash Wallet can't check this masternode's details yet. Try again in a few minutes.";
const fixed: WithdrawDestination = { kind: "fixed", address: "yPay8vK2Lw2dCsWHrPJeS4tYGmEGQnWm4c" };
const checking = (reason: Extract<WithdrawDestination, { kind: "checking" }>["reason"]): WithdrawDestination => ({ kind: "checking", reason });

describe("the evonode withdrawal over unverified data (H-09)", () => {
  it("shows the address once fixed, Checking… while it is not", () => {
    expect(withdrawDestinationText(fixed)).toBe(fixed.address);
    expect(withdrawDestinationText(checking("unverified"))).toBe("Checking…");
    expect(withdrawDestinationText(checking("spv_unsynced"))).toBe("Checking…");
  });

  it("says the masternode's details can't be checked yet under an unverified destination only", () => {
    expect(destinationNote(checking("unverified"))).toBe(UNVERIFIED);
    expect(destinationNote(checking("platform_behind"))).toBeNull();
    expect(destinationNote(fixed)).toBeNull();
    expect(destinationNote(null)).toBeNull();
  });

  it("maps withdrawal_unverified, and destination_checking whose destination is unverified", () => {
    expect(withdrawUnverifiedText(err("masternodes.withdrawal_unverified"))).toBe(UNVERIFIED);
    expect(withdrawUnverifiedText(err("masternodes.withdrawal_destination_checking"), checking("unverified"))).toBe(UNVERIFIED);
    expect(withdrawUnverifiedText(err("masternodes.withdrawal_destination_checking"), checking("payout_change_pending"))).toBeNull();
    expect(withdrawUnverifiedText(err("masternodes.withdrawal_destination_checking"))).toBeNull();
    expect(withdrawUnverifiedText(err("platform.context_unavailable"))).toBeNull();
  });
});
