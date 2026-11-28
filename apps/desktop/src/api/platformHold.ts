// The restore's Platform and Shielded hold (B's C6 and C7): the engine's refusals of a Platform or
// Shielded funds action while it holds them, or while Dash Platform is off for the session.

import type { AppError } from "./generated/commands";
import { isAppError } from "./backend";
import { t } from "../i18n";

/** Whether the cached `restore_status` says this session runs without Dash Platform (`platformOff`). */
let sessionPlatformOff: () => boolean = () => false;

/** The app's reader of the cached `restore_status`, set once at boot. */
export function readPlatformOffFrom(read: () => boolean) {
  sessionPlatformOff = read;
}

/** C7: Dash Platform is off for this session. The engine says so with `feature_off` for `"platform"`,
 * or `platform.unavailable` with the same feature. Older bare refusals can use the restore status. */
const platformOff = (e: AppError) => {
  const platform = e.data?.kind === "feature" && e.data.feature === "platform";
  return (e.code === "platform.feature_off" && platform) || (e.code === "platform.unavailable" && (platform || sessionPlatformOff()));
};

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
