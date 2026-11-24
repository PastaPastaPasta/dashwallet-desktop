// Pay ▸ To a contact ▸ From (C-funding D-C5, P13–P16): the Dash wallet, the Shielded balance or the
// Platform balance, each with its balance and, when it cannot pay, its one reason. Dash wallet is
// chosen every time the form opens and the choice is never kept (P16). A non-Core source is one
// engine call (`contactPayQuote`, then `contactPay` under a `platformOp` grant the quote sizes);
// its flow is kept per contact while the app runs, so leaving Pay while it runs keeps its result.

import type { Backend } from "../../api/backend";
import { isAppError } from "../../api/backend";
import type { AuthGrant } from "../../api/generated/AuthGrant";
import type { ContactPayOutcomeInfo } from "../../api/generated/ContactPayOutcomeInfo";
import type { ContactPayQuoteInfo } from "../../api/generated/ContactPayQuoteInfo";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { PaymentSourceName } from "../../api/generated/PaymentSourceName";
import type { PoolReading } from "../../format/homeBalance";
import { t } from "../../i18n";
import { failureText, Model } from "../../onboarding/model";
import { moveErrorText, onMoveSettledNotify } from "../../platform/move";
import { GrantModel, PAYMENT_PROMPT } from "../../tools/grant";

export type PaySource = PaymentSourceName;

/** The sources this build's engine pays a contact from. Shielded reads "Not available on this
 * network yet." until its slice lands. */
export const LIVE_SOURCES: ReadonlySet<PaySource> = new Set<PaySource>(["core", "platform"]);

const CORE_ONLY: ReadonlySet<PaySource> = new Set<PaySource>(["core"]);

/** The sources a network pays from: mainnet keeps the balances off until their funds review (the
 * engine refuses them there as `platform.feature_off`). */
export const liveSources = (network: NetworkInfo["kind"]): ReadonlySet<PaySource> => (network === "mainnet" ? CORE_ONLY : LIVE_SOURCES);

export interface FromInputs {
  /** What the Dash wallet can spend, in duffs; `undefined` while unread. */
  coreDuffs: number | undefined;
  shielded: PoolReading;
  platform: PoolReading;
  /** Duffs with the unit. */
  amount: (duffs: number) => string;
  discreet: boolean;
  /** Review accepts the payee's request too: that payment is the Dash wallet's. */
  acceptsToo: boolean;
  live?: ReadonlySet<PaySource>;
}

export interface FromRow {
  source: PaySource;
  label: string;
  /** The balance with its unit; `null` while unread or in discreet mode. */
  balance: string | null;
  /** Why this source cannot pay now; `null` when it can. */
  reason: string | null;
}

const LABEL: Record<PaySource, string> = {
  get core() {
    return t("pay.contact.from.core");
  },
  get shielded() {
    return t("pay.contact.from.shielded");
  },
  get platform() {
    return t("pay.contact.from.platform");
  },
};

export const sourceLabel = (source: PaySource): string => LABEL[source];

/** One row per source in the design's order: Dash wallet, Shielded, Platform. */
export function fromRows(i: FromInputs): FromRow[] {
  const live = i.live ?? LIVE_SOURCES;
  const shown = (duffs: number | undefined) => (duffs === undefined || i.discreet ? null : i.amount(duffs));
  const pool = (source: "shielded" | "platform", reading: PoolReading): FromRow => {
    const duffs = typeof reading === "number" ? reading : reading === "absent" ? 0 : undefined;
    const reason = !live.has(source) ? t("pay.contact.from.notOffered") : i.acceptsToo ? t("pay.contact.from.acceptsToo") : duffs === 0 ? t("pay.contact.from.empty") : null;
    return { source, label: LABEL[source], balance: shown(duffs), reason };
  };
  return [{ source: "core", label: LABEL.core, balance: shown(i.coreDuffs), reason: null }, pool("shielded", i.shielded), pool("platform", i.platform)];
}

/** The source a form pays from: the chosen one while it can pay, else the Dash wallet. */
export const payingSource = (rows: readonly FromRow[], chosen: PaySource): PaySource => (rows.find((r) => r.source === chosen)?.reason === null ? chosen : "core");

/** A quote's or a payment's refusal, as the D-C5 Copy table says it. `credits` is what was asked. */
export function fromErrorText(e: unknown, credits: string | null, amount: (credits: string) => string): string {
  if (!isAppError(e)) return failureText(e);
  switch (e.code) {
    case "credits.withdrawal_out_of_range":
      if (e.data?.kind === "creditRange" && credits !== null && BigInt(credits) < BigInt(e.data.min)) return t("pay.contact.from.belowMinimum", { min: amount(e.data.min) });
      return moveErrorText(e, amount);
    case "credits.move_pending":
      return t("pay.contact.from.movePending");
    case "platform.feature_off":
      return t("pay.contact.from.notOffered");
    case "contact.payment_locked":
      return t("pay.contact.banner.paymentLock");
    default:
      return moveErrorText(e, amount);
  }
}

/** What the result says of an outcome. */
export function outcomeText(state: ContactPayOutcomeInfo["state"], name: string): { title: string; text: string } {
  switch (state) {
    case "sent":
      return { title: t("pay.result.title.sent"), text: t("pay.contact.from.sent", { name }) };
    case "pending":
      return { title: t("pay.result.title.unknown"), text: t("pay.contact.from.unknown") };
    case "notSent":
      return { title: t("pay.result.title.failed"), text: t("pay.contact.from.notSent") };
  }
}

export type ContactPayApi = Pick<Backend, "contactPay" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly">;

/** `lost`: the call failed after it was sent, without a definitive refusal: it may have paid. */
export type FromStep = "confirm" | "running" | "done" | "refused" | "lost";

/** A refusal the engine answered before anything was signed (every coded refusal but `internal`). */
const definitive = (e: unknown) => isAppError(e) && e.code !== "internal";

/** One payment from a balance: the grant the quote sizes, then `contactPay` (one call at a time). */
export class ContactPayFlow extends Model {
  step: FromStep = "confirm";
  outcome: ContactPayOutcomeInfo | null = null;
  error: unknown = null;
  running = false;
  readonly grants: GrantModel;

  constructor(
    private readonly api: ContactPayApi,
    readonly walletId: string,
    readonly identity: string,
    readonly contact: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.prompt = PAYMENT_PROMPT;
    this.grants.subscribe(() => this.changed());
  }

  /** Back to the form: drops a seen result or refusal (not a running call). */
  reset() {
    if (this.running) return;
    this.step = "confirm";
    this.outcome = null;
    this.error = null;
    this.grants.cancel();
    this.changed();
  }

  async pay(quote: ContactPayQuoteInfo, note: string | null): Promise<void> {
    if (this.running) return;
    let ran = false;
    this.running = true;
    this.error = null;
    this.changed();
    try {
      const grant = { kind: "platformOp" as const, maxDuffs: quote.grant.maxDuffs, maxCredits: quote.grant.maxCredits };
      const got = await this.grants.withGrant(grant, this.walletId, (g: AuthGrant) => {
        ran = true;
        this.step = "running";
        this.changed();
        return this.api.contactPay(this.walletId, this.identity, this.contact, { credits: quote.credits, source: quote.source, note, confirmUnrecorded: false }, g.id);
      });
      if (got) {
        this.outcome = got.value;
        this.step = "done";
      } else this.step = "confirm";
    } catch (e) {
      this.error = e;
      this.step = ran && !definitive(e) ? "lost" : "refused";
    } finally {
      this.running = false;
      // Balances, Activity and the contact's lock follow a payment as they follow a move.
      if (ran) onMoveSettledNotify();
      this.changed();
    }
  }
}

const flows = new Map<string, ContactPayFlow>();

/** The contact's payment flow, created on first use and kept while the app runs. */
export function contactPayFor(api: ContactPayApi, walletId: string, identity: string, contact: string): ContactPayFlow {
  const key = `${walletId}:${identity}:${contact}`;
  let f = flows.get(key);
  if (!f) {
    f = new ContactPayFlow(api, walletId, identity, contact);
    flows.set(key, f);
  }
  return f;
}

/** Tests: forget every flow (a fresh app). */
export function resetContactPays() {
  flows.clear();
}
