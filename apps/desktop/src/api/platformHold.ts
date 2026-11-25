// The restore's Platform and Shielded hold (B's C6 and C7): the engine's refusals of a Platform or
// Shielded funds action while it holds them, or while Dash Platform is off for the session.

import type { AppError } from "./generated/commands";
import { isAppError } from "./backend";
import { t } from "../i18n";

/** Left across the page boot after Finish without Dash Platform; any other boot clears it, and a new
 * start has no session storage. */
const OPENED_WITHOUT_PLATFORM = "dw-opened-without-platform";

export function leaveOpenedWithoutPlatform(on: boolean) {
  try {
    if (on) sessionStorage.setItem(OPENED_WITHOUT_PLATFORM, "1");
    else sessionStorage.removeItem(OPENED_WITHOUT_PLATFORM);
  } catch {
    // No storage: the next page reads Platform's refusals as unreachable.
  }
}

function openedWithoutPlatform(): boolean {
  try {
    return sessionStorage.getItem(OPENED_WITHOUT_PLATFORM) === "1";
  } catch {
    return false;
  }
}

/** C7: Dash Platform is off for this session. The engine says so with `feature_off` for `"platform"`,
 * or with a bare `platform.unavailable` that only this session's own open tells from unreachable
 * (`platformOnly` doesn't: it outlives the session into the next one, which has Platform). */
const platformOff = (e: AppError) =>
  (e.code === "platform.feature_off" && e.data?.kind === "feature" && e.data.feature === "platform") ||
  (e.code === "platform.unavailable" && openedWithoutPlatform());

/** C7's line alone, for a refusal Platform's hold of the restore never makes; null for any other. */
export function platformOffText(e: unknown, sending = true): string | null {
  if (!isAppError(e) || !platformOff(e)) return null;
  return t(sending ? "storeUpgrade.restore.platformOff" : "storeUpgrade.restore.platformOffNotSending");
}

/** The line for a Platform or Shielded funds action the restore's hold refused (C6), or that Dash
 * Platform being off this session refused (C7); null for any other failure. `sending` is false where
 * nothing was being sent (a quote, a Max, an address, a check). */
export function platformHoldText(e: unknown, sending = true): string | null {
  if (isAppError(e) && e.code === "platform.restored") return t("storeUpgrade.restore.platformRestored");
  return platformOffText(e, sending);
}
