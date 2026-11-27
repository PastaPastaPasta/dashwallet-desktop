// Settings ▸ Advanced ▸ Connected apps (DashPay App Connect): lists each app this wallet approved
// for the current identity that still has enabled session keys on Dash Platform, and runs
// Disconnect (quote with live Platform credit fee, passphrase-required `appDisconnect` grant,
// MASTER-signed IdentityUpdate disabling only that app's verified session keys).

import { useQuery } from "@tanstack/react-query";
import type { Backend } from "../api/backend";
import { isAppError } from "../api/backend";
import type { ConnectBinding } from "../api/generated/ConnectBinding";
import type { ConnectFailureName } from "../api/generated/ConnectFailureName";
import type { ConnectedApp } from "../api/generated/ConnectedApp";
import type { ConnectedAppKey } from "../api/generated/ConnectedAppKey";
import type { DisconnectDone } from "../api/generated/DisconnectDone";
import type { DisconnectQuote } from "../api/generated/DisconnectQuote";
import { useBackend } from "../api/queries";
import { t } from "../i18n";
import { Model } from "../onboarding/model";
import { GrantModel } from "../tools/grant";
import { bindingText, criticalText } from "./connect";
import { creditCount } from "./profile";

export type ConnectedAppsApi = Pick<
  Backend,
  | "appConnectList"
  | "appConnectDisconnectQuote"
  | "appConnectDisconnect"
  | "authorize"
  | "revokeGrant"
  | "lockStatus"
  | "paymentAuthentication"
  | "unlock"
  | "unlockMixingOnly"
>;

export const connectedAppsKeys = {
  list: (walletId: string, identity: string) => ["dashpay", "connectedApps", walletId, identity] as const,
};

/** Connected apps for `identity` on a test network; disabled on mainnet or when `identity` is null. */
export function useConnectedApps(walletId: string, identity: string | null, enabled: boolean) {
  const b = useBackend();
  return useQuery({
    queryKey: connectedAppsKeys.list(walletId, identity ?? ""),
    queryFn: () => b.appConnectList(walletId, identity!),
    enabled: enabled && identity !== null,
    retry: false,
  });
}

function asBinding(key: ConnectedAppKey): ConnectBinding {
  return { index: 0, primary: true, level: key.level, scope: key.scope };
}

/** Scope summary for one still-enabled app session key, reusing the sign-in sheet's wording. */
export function connectedKeyText(key: ConnectedAppKey): string {
  return bindingText(asBinding(key));
}

/** Critical-key warning for one still-enabled app session key, when applicable. */
export function connectedKeyCritical(key: ConnectedAppKey): string | null {
  return criticalText(asBinding(key));
}

/** Errors the engine provably raised before any IdentityUpdate was sent. */
const UNSENT_CODES = new Set([
  "platform.grant_invalid",
  "platform.signer_unavailable",
  "platform.seed_mismatch",
  "platform.needs_grant",
  "platform.cancelled",
  "platform.lease_revoked",
  "platform.lease_expired",
  "invalid_argument",
  "identity.not_found",
  "identity.keys_missing",
]);

function connectFailureName(data: unknown): ConnectFailureName | null {
  if (typeof data !== "object" || data === null) return null;
  const d = data as { kind?: unknown; failure?: unknown };
  return d.kind === "connectFailure" && typeof d.failure === "string" ? (d.failure as ConnectFailureName) : null;
}

/** Plain-language error for a Disconnect quote or action refusal. */
export function disconnectErrorText(e: unknown): string {
  if (!isAppError(e)) return t("dashpay.connect.connected.err.generic");
  if (e.code === "platform.restored") return t("dashpay.connect.err.platformRestored");
  if (e.code === "platform.unavailable") return t("dashpay.connect.err.platformUnavailable");
  if (e.code === "connect.mainnet_off") return t("dashpay.connect.err.mainnetOff");
  if (e.code === "platform.grant_exceeded") return t("dashpay.connect.connected.err.grantExceeded");
  if (e.code === "connect.credits" && e.data?.kind === "credits") {
    const short = BigInt(e.data.needed) - BigInt(e.data.available);
    return t("dashpay.connect.connected.lowCredits", { amount: creditCount(short > 0n ? short : 0n) });
  }
  if (e.code === "connect.failed") {
    const failure = connectFailureName(e.data);
    if (failure === "notConfirmed") return t("dashpay.connect.connected.err.notConfirmed");
    if (failure === "unverifiedKey") return t("dashpay.connect.connected.err.unverifiedKey");
    if (failure === "feeTooHigh") return t("dashpay.connect.connected.err.feeTooHigh");
    if (failure === "disconnectConflict") return t("dashpay.connect.connected.err.disconnectConflict");
    if (failure === "pendingFeeTooHigh") return t("dashpay.connect.connected.err.pendingFeeTooHigh");
    if (failure === "pendingWrite") return t("dashpay.connect.connected.err.pendingWrite");
  }
  if (UNSENT_CODES.has(e.code)) return t("dashpay.connect.connected.err.genericUnsent");
  return t("dashpay.connect.connected.err.generic");
}

/** Formats an epoch-ms timestamp in the backend's locale. */
export function formatConnectDate(ms: number, locale?: string): string {
  return new Date(ms).toLocaleDateString(locale, { year: "numeric", month: "short", day: "numeric" });
}

export class ConnectedAppsModel extends Model {
  target: ConnectedApp | null = null;
  quote: DisconnectQuote | null = null;
  quoting = false;
  requoted = false;
  quoteError: unknown = null;
  disconnecting = false;
  disconnectError: unknown = null;
  disconnectedLabel: string | null = null;
  lastDone: DisconnectDone | null = null;
  readonly grants: GrantModel;
  readonly confirmedLegacyKeys = new Set<number>();
  #quoteSeq = 0;

  constructor(
    private readonly api: ConnectedAppsApi,
    readonly walletId: string,
    private readonly onDisconnected: () => void,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.subscribe(() => this.changed());
  }

  get legacyKeysConfirmed(): boolean {
    const quote = this.quote;
    return !quote?.needsLegacyKeyConfirmation || quote.keys.every((key) => this.confirmedLegacyKeys.has(key));
  }

  confirmLegacyKey(key: number, checked: boolean): void {
    if (checked) this.confirmedLegacyKeys.add(key);
    else this.confirmedLegacyKeys.delete(key);
    this.changed();
  }

  /** Opens the Disconnect confirmation sheet for `app` and reads the live Platform credit fee. */
  async openDisconnect(app: ConnectedApp): Promise<void> {
    if (this.disconnecting) return;
    const seq = ++this.#quoteSeq;
    this.target = app;
    this.quote = null;
    this.confirmedLegacyKeys.clear();
    this.quoting = true;
    this.requoted = false;
    this.quoteError = null;
    this.disconnectError = null;
    this.disconnectedLabel = null;
    this.changed();
    try {
      const q = await this.api.appConnectDisconnectQuote(this.walletId, app.identity, app.request);
      if (seq !== this.#quoteSeq || this.target?.request !== app.request) return;
      this.quote = q;
      this.quoting = false;
      this.changed();
    } catch (e) {
      if (seq !== this.#quoteSeq || this.target?.request !== app.request) return;
      this.quoting = false;
      this.quoteError = e;
      this.changed();
    }
  }

  /** Closes the confirmation sheet (blocked while the update is in flight). */
  closeDisconnect(): void {
    if (this.disconnecting) return;
    this.#quoteSeq++;
    this.grants.cancel();
    this.target = null;
    this.quote = null;
    this.quoting = false;
    this.requoted = false;
    this.quoteError = null;
    this.disconnectError = null;
    this.changed();
  }

  /** Prompts for the passphrase (`appDisconnect` grant) and disables the target app's keys. */
  async confirmDisconnect(): Promise<void> {
    const app = this.target;
    const q = this.quote;
    if (!app || !q || this.quoting || this.disconnecting || !this.legacyKeysConfirmed) return;
    this.disconnectError = null;
    this.requoted = false;
    this.changed();
    const epoch = this.grants.epoch;
    let grant;
    try {
      const purpose = q.grant.kind === "appDisconnect" && q.needsLegacyKeyConfirmation
        ? { ...q.grant, confirmedLegacyKeys: true }
        : q.grant;
      grant = await this.grants.obtain(purpose, this.walletId, epoch);
    } catch (e) {
      if (this.target?.request === app.request) {
        this.disconnectError = e;
        this.changed();
      }
      return;
    }
    if (!grant) return;
    this.disconnecting = true;
    this.changed();
    try {
      const done = await this.api.appConnectDisconnect(this.walletId, app.identity, app.request, grant.id);
      this.disconnecting = false;
      this.target = null;
      this.quote = null;
      this.quoteError = null;
      this.disconnectError = null;
      this.disconnectedLabel = app.label;
      this.lastDone = done;
      this.changed();
      this.onDisconnected();
    } catch (e) {
      this.grants.revoke(grant);
      this.disconnecting = false;
      this.disconnectError = e;
      const failure = isAppError(e) && e.code === "connect.failed" ? connectFailureName(e.data) : null;
      const shouldRefresh = failure === "notConfirmed" || failure === "pendingFeeTooHigh" || failure === "disconnectConflict";
      const shouldRequote =
        (isAppError(e) && (e.code === "platform.grant_exceeded" || e.code === "platform.grant_invalid")) ||
        failure === "feeTooHigh" ||
        failure === "pendingFeeTooHigh" ||
        failure === "disconnectConflict";
      if (shouldRefresh) this.onDisconnected();
      if (shouldRequote) {
        await this.#requote(app);
      } else {
        this.changed();
      }
    }
  }

  async #requote(app: ConnectedApp): Promise<void> {
    const seq = ++this.#quoteSeq;
    this.quote = null;
    this.confirmedLegacyKeys.clear();
    this.quoting = true;
    this.quoteError = null;
    this.changed();
    try {
      const q = await this.api.appConnectDisconnectQuote(this.walletId, app.identity, app.request);
      if (seq !== this.#quoteSeq || this.target?.request !== app.request) return;
      this.quote = q;
      this.quoting = false;
      this.requoted = true;
      this.changed();
    } catch (qe) {
      if (seq !== this.#quoteSeq || this.target?.request !== app.request) return;
      this.quoting = false;
      this.quoteError = qe;
      this.changed();
    }
  }
}
