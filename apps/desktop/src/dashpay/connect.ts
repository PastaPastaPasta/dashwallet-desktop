// "Sign in to an app" (DashPay App Connect): the sheet of one request dw-app keeps by its inbox id,
// the identity, keys and funding limit the user picks, the quote, and the approval under an
// `appConnect` grant, which takes the passphrase every time. The request itself and its return link
// never reach the renderer: every call names the inbox id, and dw-app opens the return link.
// Refusals are said by their code alone, never by the engine's message.

import type { Backend } from "../api/backend";
import { hasCode, isAppError } from "../api/backend";
import type { ConnectBinding } from "../api/generated/ConnectBinding";
import type { ConnectChoice } from "../api/generated/ConnectChoice";
import type { ConnectDone } from "../api/generated/ConnectDone";
import type { ConnectFailureName } from "../api/generated/ConnectFailureName";
import type { ConnectQuote } from "../api/generated/ConnectQuote";
import type { ConnectSheet } from "../api/generated/ConnectSheet";
import type { ErrorCode } from "../api/generated/ErrorCode";
import { messageTable, t } from "../i18n";
import { networkName } from "../network/choice";
import { Model } from "../onboarding/model";
import { GrantModel } from "../tools/grant";
import { CREDITS_PER_DUFF } from "../api/pending";
import { dashText, parseDash } from "../masternodes/shared";
import { creditCount } from "./profile";

export type ConnectApi = Pick<
  Backend,
  | "appConnectSheet"
  | "appConnectQuote"
  | "appConnectApprove"
  | "appConnectRetrySend"
  | "appConnectDeny"
  | "appConnectReturn"
  | "appConnectDismiss"
  | "authorize"
  | "revokeGrant"
  | "lockStatus"
  | "paymentAuthentication"
  | "unlock"
  | "unlockMixingOnly"
>;

const FAILURE_NAMES = [
  "keyConflict",
  "notConfirmed",
  "noResponseKey",
  "nothingToSend",
  "scopeChanged",
  "unverifiedKey",
  "feeTooHigh",
  "disconnectConflict",
  "pendingFeeTooHigh",
  "pendingWrite",
] as const satisfies readonly ConnectFailureName[];

/** `connect.failed`'s detail, by where the approval stopped; `unknown` for anything else. */
const FAILURE: Readonly<Record<ConnectFailureName | "unknown", string>> = messageTable("dashpay.connect.failure", [...FAILURE_NAMES, "unknown"]);

/** What the engine proved about the identity when each failure stopped the approval: `scopeChanged`
 * comes only after the keys were read back on the identity. A `keyConflict` key can only come from
 * an earlier approval of the same request, and `noResponseKey` is raised both before and after the
 * update, so those, like the rest, say nothing. */
export const FAILURE_KEYS: Readonly<Record<ConnectFailureName | "unknown", "added" | null>> = {
  keyConflict: null,
  notConfirmed: null,
  noResponseKey: null,
  nothingToSend: null,
  scopeChanged: "added",
  unverifiedKey: null,
  feeTooHigh: null,
  disconnectConflict: null,
  pendingFeeTooHigh: null,
  pendingWrite: null,
  unknown: null,
};

function failureName(data: unknown): ConnectFailureName | "unknown" {
  const failure = typeof data === "object" && data !== null && (data as { kind?: unknown }).kind === "connectFailure" ? (data as { failure?: unknown }).failure : null;
  return (FAILURE_NAMES as readonly unknown[]).includes(failure) ? (failure as ConnectFailureName) : "unknown";
}

/** The detail `connect.failed` names: from the fixed list only, never from the error's text. */
export function failureDetail(data: unknown): string {
  return FAILURE[failureName(data)];
}

/** `connect.failed`'s whole text: the detail, then what is on the identity where that is known. */
function failedText(data: unknown): string {
  const keys = FAILURE_KEYS[failureName(data)];
  const text = t("dashpay.connect.err.failed", { detail: failureDetail(data) });
  return keys === null ? text : `${text} ${t("dashpay.connect.failed.added")}`;
}

/** Each refusal that says no more than its code. */
const BY_CODE: Partial<Record<ErrorCode, string>> = {
  get "connect.unsupported"() {
    return t("dashpay.connect.err.unsupported");
  },
  get "connect.invalid"() {
    return t("dashpay.connect.err.invalid");
  },
  get "connect.stale"() {
    return t("dashpay.connect.err.stale");
  },
  get "connect.protocol"() {
    return t("dashpay.connect.err.protocol");
  },
  get "connect.no_identity"() {
    return t("dashpay.connect.err.noIdentity");
  },
  get "connect.hint"() {
    return t("dashpay.connect.err.hint");
  },
  get "connect.owner"() {
    return t("dashpay.connect.err.owner");
  },
  get "connect.answered"() {
    return t("dashpay.connect.err.answered");
  },
  get "connect.send_failed"() {
    return t("dashpay.connect.sendFailed");
  },
};

/** What a refusal says, for the window's network `network` (a directory name). */
export function connectErrorText(e: unknown, network: string): string {
  if (!isAppError(e)) return t("dashpay.connect.err.check");
  if (e.code === "platform.restored") return t("dashpay.connect.err.platformRestored");
  if (e.code === "platform.unavailable") return t("dashpay.connect.err.platformUnavailable");
  if (e.code === "connect.network" && e.data?.kind === "connectNetwork")
    return t("dashpay.connect.err.network", { requested: networkName(e.data.requested), current: networkName(network) });
  if (e.code === "connect.failed") return failedText(e.data);
  if (e.code === "connect.credits" && e.data?.kind === "credits") {
    const short = BigInt(e.data.needed) - BigInt(e.data.available);
    return t("dashpay.connect.err.credits", { amount: creditCount(short > 0n ? short : 0n) });
  }
  return BY_CODE[e.code] ?? t("dashpay.connect.err.check");
}

/** A Platform error that may pass: a read of the request that failed with one keeps the sheet. */
const temporary = (e: unknown) => hasCode(e, "platform.timeout") || hasCode(e, "platform.unavailable");

/** A `connect.*` refusal answered the request, except a credit shortfall: the sheet stays open. */
const ends = (e: unknown) => isAppError(e) && e.code.startsWith("connect.") && e.code !== "connect.credits" && !(e.code === "connect.failed" && failureName(e.data) === "notConfirmed");

/** What a binding's row says. */
export function bindingText(b: ConnectBinding): string {
  switch (b.scope.kind) {
    case "contract":
      return t("dashpay.connect.grant.contract");
    case "documentType":
      return t("dashpay.connect.grant.docType", { type: b.scope.documentType });
    case "profile":
      return t("dashpay.connect.grant.profile");
    case "group": {
      const count = b.scope.group.contracts.length;
      return b.scope.group.admins.length > 0 ? t("dashpay.connect.grant.groupAdmins", { count }) : t("dashpay.connect.grant.group", { count });
    }
  }
}

/** Whole days from the sheet's block time to the terms' end, at least 1. */
export function expiryDays(sheet: Pick<ConnectSheet, "blockTimeMs" | "terms">): number {
  return Math.max(1, Math.round((sheet.terms.expiresAtMs - sheet.blockTimeMs) / 86_400_000));
}

/** The funding limit as typed, in Dash, as credits; `null` when it isn't a positive amount or has
 * more than 8 decimals. */
export const limitCredits = (text: string): bigint | null => {
  const duffs = parseDash(text);
  return duffs === null || duffs <= 0 ? null : BigInt(duffs) * CREDITS_PER_DUFF;
};
/** A limit as typed is valid up to `max` credits, the sheet's `maxFundingLimit`. */
export const validLimit = (text: string, max: string) => {
  const credits = limitCredits(text);
  return credits !== null && credits <= BigInt(max);
};
/** The limit is quoted once typing pauses: each quote reads Platform. */
const LIMIT_QUOTE_DELAY_MS = 400;
export const limitText = (credits: string) => dashText(Number(BigInt(credits) / CREDITS_PER_DUFF));

const CREDITS_PER_DASH = 100_000_000_000n;
/** Credits as Dash, every digit kept: what the quote writes to Platform. */
export function creditsDash(credits: bigint): string {
  const frac = String(credits % CREDITS_PER_DASH).padStart(11, "0").replace(/0+$/, "");
  return frac ? `${credits / CREDITS_PER_DASH}.${frac}` : String(credits / CREDITS_PER_DASH);
}
const planOf = (quote: ConnectQuote | null) => (quote?.grant.kind === "appConnect" ? quote.grant.plan : null);

/** The funding limit the quote writes to Platform: its keys' budgets together. */
export const quotedLimit = (quote: ConnectQuote) => creditsDash(quote.budgets.reduce((sum, b) => sum + BigInt(b), 0n));

/** Only a CRITICAL key can sign a token transition, and only one bound to a whole contract or a
 * group can reach tokens; a group always counts, since its owner can add contracts with tokens. */
const signsTokens = (b: ConnectBinding) => b.level === "critical" && (b.scope.kind === "group" || (b.scope.kind === "contract" && b.scope.tokens));

export function criticalText(b: ConnectBinding): string | null {
  if (b.level !== "critical") return null;
  return signsTokens(b) ? t("dashpay.connect.critical") : t("dashpay.connect.criticalNoTokens");
}

/** A document type can charge a token cost, even in another contract's token, which any key's
 * document writes pay; the DashPay profile charges none, and the critical warning already names
 * tokens where the key can sign token transitions. */
export function tokenCostText(b: ConnectBinding): string | null {
  return b.scope.kind !== "profile" && !signsTokens(b) ? t("dashpay.connect.tokenCost") : null;
}

export type ConnectPhase = "loading" | "sheet" | "approving" | "sendFailed" | "sending" | "done" | "denied" | "refused";

export class ConnectFlow extends Model {
  phase: ConnectPhase = "loading";
  sheet: ConnectSheet | null = null;
  identity: string | null = null;
  /** The funding limit as typed, in Dash. */
  limit = "";
  quote: ConnectQuote | null = null;
  quoteError: unknown = null;
  quoting = false;
  /** Approve, Deny or Return is being answered. */
  busy = false;
  /** Why the sheet ended; `null` while it did not. */
  error: unknown = null;
  /** The request changed on Platform during an approval: the sheet was read again. */
  planChanged = false;
  /** A retry of the send failed again. */
  retryFailed = false;
  /** The return link did not open. */
  returnFailed = false;
  /** An approval of this request reached the engine and did not finish, here or on an earlier
   * sheet: keys may be on the identity, so the choice is kept as it was approved and Deny claims
   * nothing about them. */
  attempted: ConnectChoice | null = null;
  done: ConnectDone | null = null;
  readonly grants: GrantModel;
  /** The non-primary bindings left out, by index. */
  #off = new Set<number>();
  #typing: ReturnType<typeof setTimeout> | undefined;
  #quotes = 0;
  #loads = 0;
  /** Quotes in a row that read other group facts than the sheet shows, since one that matched. */
  #mismatches = 0;
  /** Try again reads the request first: a quote read other group facts again, or that read failed. */
  #refresh = false;
  #closed = false;

  constructor(
    private readonly api: ConnectApi,
    readonly walletId: string,
    readonly linkId: string,
    readonly returnsTo: string | null,
    private readonly onClose: () => void,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.subscribe(() => this.changed());
  }

  get closed() {
    return this.#closed;
  }

  /** Reads the sheet; a sheet read again keeps what the user picked where it still applies. */
  async load(): Promise<void> {
    const n = ++this.#loads;
    const again = this.sheet !== null;
    this.phase = "loading";
    this.quote = null;
    this.changed();
    try {
      const sheet = await this.api.appConnectSheet(this.walletId, this.linkId);
      if (n !== this.#loads || this.#closed) return;
      this.sheet = sheet;
      if (!sheet.identities.some((i) => i.identity === this.identity)) this.identity = sheet.identities[0]?.identity ?? null;
      this.#off = new Set(sheet.bindings.filter((b) => !b.primary && this.#off.has(b.index)).map((b) => b.index));
      if (!again || !this.limitValid()) this.limit = limitText(sheet.terms.fundingLimit);
      const earlier = this.attempted ?? sheet.earlierApproval;
      if (earlier) {
        this.attempted = earlier;
        this.identity = earlier.identity;
        this.#off = new Set(sheet.bindings.filter((b) => !b.primary && !earlier.selection.includes(b.index)).map((b) => b.index));
        this.limit = limitText(earlier.terms.fundingLimit);
      }
      this.#refresh = false;
      this.phase = "sheet";
      this.changed();
      void this.requote();
    } catch (e) {
      if (n !== this.#loads || this.#closed) return;
      if (!again || !temporary(e)) return this.#fail(e);
      clearTimeout(this.#typing);
      this.#quotes++;
      this.quoting = false;
      this.#refresh = true;
      this.quoteError = e;
      this.phase = "sheet";
      this.changed();
    }
  }

  /** The choice may still change: the sheet is open, no approval of it is outstanding, and it needs
   * no read of the request first (Try again's). */
  get editable(): boolean {
    return this.phase === "sheet" && !this.busy && this.attempted === null && !this.#refresh;
  }

  /** The limit as typed is within the sheet's maximum. */
  limitValid(): boolean {
    return this.sheet !== null && validLimit(this.limit, this.sheet.maxFundingLimit);
  }

  selected(b: ConnectBinding): boolean {
    return b.primary || !this.#off.has(b.index);
  }

  /** Ticks or unticks a binding; the primary one stays. */
  toggle(b: ConnectBinding) {
    if (b.primary || !this.editable) return;
    if (this.#off.has(b.index)) this.#off.delete(b.index);
    else this.#off.add(b.index);
    void this.requote();
  }

  setIdentity(identity: string) {
    if (!this.editable) return;
    this.identity = identity;
    void this.requote();
  }

  setLimit(text: string) {
    if (!this.editable) return;
    this.limit = text;
    clearTimeout(this.#typing);
    this.#quotes++;
    this.quote = null;
    this.quoteError = null;
    this.quoting = this.limitValid();
    this.#typing = setTimeout(() => void this.requote(), LIMIT_QUOTE_DELAY_MS);
    this.changed();
  }

  /** What Approve approves: `null` until an identity is picked and the limit is valid; after an
   * approval that did not finish, that approval's choice. */
  choice(): ConnectChoice | null {
    if (this.attempted) return this.attempted;
    const { sheet, identity } = this;
    if (!sheet || identity === null || !this.limitValid()) return null;
    return {
      identity,
      selection: sheet.bindings.filter((b) => this.selected(b)).map((b) => b.index),
      terms: { fundingLimit: String(limitCredits(this.limit)), expiresAtMs: sheet.terms.expiresAtMs },
    };
  }

  /** Quotes the current choice; only the latest quote's answer counts. A quote that read other group
   * facts than the sheet shows reads the request again, once: if the next one disagrees too, the quote
   * fails and the user's retry reads it again. */
  async requote(): Promise<void> {
    const n = ++this.#quotes;
    const choice = this.choice();
    this.quote = null;
    this.quoteError = null;
    this.quoting = choice !== null;
    this.changed();
    if (!choice) return;
    try {
      const quote = await this.api.appConnectQuote(this.walletId, this.linkId, choice);
      if (n !== this.#quotes || this.#closed) return;
      if (quote.groups !== this.sheet?.groups) {
        if (this.#mismatches++ === 0) return this.#reread();
        this.#refresh = true;
        this.quoteError = new Error("group facts changed");
        return;
      }
      this.#mismatches = 0;
      this.#refresh = false;
      this.quote = quote;
    } catch (e) {
      if (n !== this.#quotes || this.#closed) return;
      if (hasCode(e, "connect.plan_changed")) return this.#reread();
      if (ends(e)) return this.#fail(e);
      this.quoteError = e;
    } finally {
      if (n === this.#quotes) {
        this.quoting = false;
        this.changed();
      }
    }
  }

  /** The quote's Try again: after a quote whose group facts disagreed again, or a failed read of
   * the request, it reads the request first. */
  retryQuote() {
    if (this.#refresh) this.#reread();
    else void this.requote();
  }

  /** Approve: a fresh quote, its grant under the passphrase, then the approval. A fresh quote that
   * read anything other than what the sheet showed (its plan, or the group facts the sheet lists)
   * reads the request again instead. */
  async approve(): Promise<void> {
    const choice = this.choice();
    if (this.phase !== "sheet" || this.busy || !choice) return;
    this.busy = true;
    this.planChanged = false;
    this.changed();
    let approving = false;
    try {
      const quote = await this.api.appConnectQuote(this.walletId, this.linkId, choice);
      if (this.#closed) return;
      if (planOf(quote) !== planOf(this.quote) || quote.groups !== this.sheet?.groups) {
        this.busy = false;
        return this.#reread();
      }
      const got = await this.grants.withGrant(quote.grant, this.walletId, (g) => {
        this.phase = "approving";
        this.changed();
        approving = true;
        return this.api.appConnectApprove(this.walletId, this.linkId, choice, g.id);
      });
      if (this.#closed) return;
      if (got) {
        this.done = got.value;
        this.phase = "done";
      }
    } catch (e) {
      if (this.#closed) return;
      if (hasCode(e, "connect.send_failed")) this.phase = "sendFailed";
      else if (hasCode(e, "connect.plan_changed")) {
        this.busy = false;
        return this.#reread();
      } else if (ends(e)) this.#fail(e);
      else {
        if (approving) this.attempted = choice;
        this.phase = "sheet";
        this.quoteError = e;
      }
    } finally {
      this.busy = false;
      this.changed();
    }
  }

  async retrySend(): Promise<void> {
    if (this.phase !== "sendFailed") return;
    this.phase = "sending";
    this.retryFailed = false;
    this.changed();
    try {
      const done = await this.api.appConnectRetrySend(this.walletId, this.linkId);
      if (this.#closed) return;
      this.done = done;
      this.phase = "done";
    } catch (e) {
      if (this.#closed) return;
      if (hasCode(e, "connect.send_failed")) {
        this.phase = "sendFailed";
        this.retryFailed = true;
      } else this.#fail(e);
    } finally {
      this.changed();
    }
  }

  async deny(): Promise<void> {
    if (this.busy || (this.phase !== "sheet" && this.phase !== "loading")) return;
    this.busy = true;
    this.#stop();
    this.grants.cancel();
    this.changed();
    try {
      await this.api.appConnectDeny(this.linkId);
      if (this.returnsTo === null && this.attempted === null) return this.close();
      this.phase = "denied";
    } catch (e) {
      this.#fail(e);
    } finally {
      this.busy = false;
      this.changed();
    }
  }

  /** Back to the app: dw-app opens its return link with the answer; the sheet then closes. */
  async returnToApp(): Promise<void> {
    if (this.busy || this.returnsTo === null) return;
    this.busy = true;
    this.returnFailed = false;
    this.changed();
    try {
      await this.api.appConnectReturn(this.linkId);
      this.close();
    } catch {
      this.returnFailed = true;
    } finally {
      this.busy = false;
      this.changed();
    }
  }

  /** Whether the sheet may close: not while an approval or a send runs. */
  get closable(): boolean {
    return this.phase !== "approving" && this.phase !== "sending";
  }

  /** Closes the sheet and forgets the request. */
  close() {
    if (this.#closed || !this.closable) return;
    this.#closed = true;
    this.#stop();
    this.grants.cancel();
    void this.api.appConnectDismiss(this.linkId).catch(() => null);
    this.onClose();
  }

  /** No quote or read still outstanding answers. */
  #stop() {
    clearTimeout(this.#typing);
    this.#quotes++;
    this.#loads++;
    this.quoting = false;
  }

  #reread() {
    this.planChanged = true;
    void this.load();
  }

  #fail(e: unknown) {
    if (this.#closed) return;
    this.#quotes++;
    this.quoting = false;
    this.phase = "refused";
    this.error = e;
    this.changed();
  }
}
