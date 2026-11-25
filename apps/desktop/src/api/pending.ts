// What the shielded and Platform-address screens share that the generated bindings do not carry: the
// credit/duff conversion, the shielded error codes by name (typed against the generated `ShieldedCode`)
// and the "this dw-app has no such command" test the optional reads use. The commands themselves are
// dw-app's (`generated/commands.ts`): lane E2's network choice and Platform addresses (contract
// m4-dashpay-engine §2.12, §8) and lane E1's shielded funds (§2.13), integrated at m4 v47.

import type { FundAmount } from "./generated/FundAmount";
import type { ShieldedCode } from "./generated/ShieldedCode";
import { t } from "../i18n";

/** Platform's credits per duff (dpp's `CREDITS_PER_DUFF`): what `PlatformAddressBalance.duffs` and
 * the shielded pool's figures use. */
export const CREDITS_PER_DUFF = 1000n;

/** A shield's or a fund's amount: `duffs` with the fee on top, or, for a Core Max (`all`), everything
 * in the Dash wallet's account 0 with the fee out of it. */
export const fundAmount = (r: { duffs: number; all?: boolean }): FundAmount => (r.all ? { kind: "all" } : { kind: "duffs", duffs: r.duffs });

/** The refusals only a Core Max meets (`shielded.*` or `credits.*`): mixing, another move of the coins,
 * coins changed since the quote. With `max` (the request was a Core Max) also its own words for a
 * locked coin and for more than 50 coins. `null`: another refusal. */
export function coreMaxRefusal(e: unknown, max: boolean): string | null {
  if (typeof e !== "object" || e === null || !("code" in e) || typeof e.code !== "string") return null;
  // A Max's grant is exactly its quote's total: coins that grew since then exceed it.
  if (max && e.code === "platform.grant_exceeded") return t("transfer.maxChanged");
  const [domain, code] = e.code.split(".");
  if (domain !== "shielded" && domain !== "credits") return null;
  const data = "data" in e ? (e.data as { kind?: string; value?: unknown } | null) : null;
  switch (code) {
    case "mixing_active":
      return t("transfer.maxMixing");
    case "move_running":
      return t("transfer.maxMoveRunning");
    case "coins_changed":
      return t("transfer.maxChanged");
    case "coins_locked":
      return max ? t("transfer.maxLocked") : null;
    case "too_many_inputs":
      return max && data?.kind === "number" ? t("transfer.maxTooMany", { count: String(data.value) }) : null;
    default:
      return null;
  }
}

/** Whole duffs in `credits` (a decimal string). */
export const creditsToDuffs = (credits: string): number => Number(BigInt(credits) / CREDITS_PER_DUFF);

/** Credits as duffs, rounded up: an "about" amount or a least amount never undersells. */
export const creditsToDuffsUp = (credits: bigint | string): number => Number((BigInt(credits) + CREDITS_PER_DUFF - 1n) / CREDITS_PER_DUFF);

/** The `ShieldedCode`s by name, one place for the screens' copy and the fixture. Every value is the
 * generated `ShieldedCode`, so a code the engine adds or renames is a type error here. */
export const SHIELDED_CODES = {
  notActive: "shielded.not_active",
  notSynced: "shielded.not_synced",
  invalidAddress: "shielded.invalid_address",
  insufficientFunds: "shielded.insufficient_funds",
  fundingInsufficient: "shielded.funding_insufficient",
  belowMinimum: "shielded.below_minimum",
  coinsLocked: "shielded.coins_locked",
  shieldPending: "shielded.shield_pending",
  poolTooSmall: "shielded.pool_too_small",
  tooManyInputs: "shielded.too_many_inputs",
  mixingActive: "shielded.mixing_active",
  nothingToMove: "shielded.nothing_to_move",
  coinsUnconfirmed: "shielded.coins_unconfirmed",
  moveRunning: "shielded.move_running",
  coinsChanged: "shielded.coins_changed",
  unrecordedSpend: "shielded.unrecorded_spend",
} as const satisfies Record<string, ShieldedCode>;

/** Whether `e` says the command does not exist in this dw-app. dw-app's ACL manifest lists every
 * command it has, so Tauri answers an unknown one from the ACL check: "Command x not allowed by
 * ACL" in a release build, "x not allowed. Command not found" in a debug build; "Command x not
 * found" is the handler lookup's. A command that exists but has no capability is absent the same. */
export function commandAbsent(e: unknown): boolean {
  if (typeof e === "string") return /^command .* (not found|not allowed by acl)/i.test(e) || /not allowed\. command not found/i.test(e);
  return typeof e === "object" && e !== null && "code" in e && (e as { code: unknown }).code === "platform.not_implemented";
}

/** `call`'s answer, or `null` when dw-app has no such command (the screen then shows its
 * "unavailable" state). Any other failure is thrown. */
export async function orAbsent<T>(call: Promise<T>): Promise<T | null> {
  try {
    return await call;
  } catch (e) {
    if (commandAbsent(e)) return null;
    throw e;
  }
}

/** `call`'s answer; against a dw-app without the command, an error the screen shows (a save, a move of
 * funds: the call means nothing without it). */
export async function orUnavailable<T>(call: Promise<T>): Promise<T> {
  const got = await orAbsent(call);
  if (got === null) throw new Error(t("app.unavailableInBuild"));
  return got;
}
