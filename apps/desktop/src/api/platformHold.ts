// The restore's Platform and Shielded hold (B's C6 and C7): `restore_status`'s `platformOnly`, and the
// engine's refusals of a Platform or Shielded funds action while it holds them, or while Dash Platform
// is off for the session. `platformOnly` is read from the answer until the bindings carry it.

import type { AppError } from "./generated/commands";
import type { RestoreStatus } from "./generated/RestoreStatus";
import { isAppError } from "./backend";
import { t } from "../i18n";

/** C6: the Core gate is open (`restored` is false) and only Platform and Shielded funds wait. */
export type HeldRestoreStatus = RestoreStatus & { platformOnly?: boolean };

export const platformOnly = (s: RestoreStatus): boolean => (s as HeldRestoreStatus).platformOnly === true;

/** C7: Dash Platform is off for this session (`feature: "platform"`), not unreachable or off on this network. */
const platformOff = (e: AppError) =>
  (e.code === "platform.feature_off" || e.code === "platform.unavailable") && e.data?.kind === "feature" && e.data.feature === "platform";

/** The line for a Platform or Shielded funds action the restore's hold refused (C6), or that Dash
 * Platform being off this session refused (C7); null for any other failure. `sending` is false where
 * nothing was being sent (a quote, a Max, an address, a check). */
export function platformHoldText(e: unknown, sending = true): string | null {
  if (!isAppError(e)) return null;
  if (e.code === "platform.restored") return t("storeUpgrade.restore.platformRestored");
  if (platformOff(e)) return t(sending ? "storeUpgrade.restore.platformOff" : "storeUpgrade.restore.platformOffNotSending");
  return null;
}
