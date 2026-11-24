// The instant username's offer while the own contest is open (DASHPAY F5; DP1-09 D4 reversed,
// DP1-10 D13): its copy, how long a vote runs, what became of a temporary name a registration
// asked for, and what the voting card and the request's details offer. Imports nothing of
// status.ts or registration.ts, which import it. The form's checks are instantName.ts.

import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { NetworkKind } from "../api/generated/NetworkKind";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import { spanLong } from "../governance/model";
import { messageTable, t } from "../i18n";

/** `table` with `rest`'s members, its getters kept (status.ts's `withTable`, which imports this). */
const withTable = <A extends object, B extends object>(table: A, rest: B): A & B => Object.defineProperties(rest, Object.getOwnPropertyDescriptors(table)) as A & B;

export const INSTANT_COPY = withTable(messageTable("dashpay.instant", ["title", "create", "decline", "contested", "same", "retry"]), {
  /** The offer's body, a paragraph each; `duration` how long the vote runs. */
  offer: (duration: string) => [t("dashpay.instant.offer", { duration }), t("dashpay.instant.offerMore")],
  explainer: (requested: string) => t("dashpay.instant.explainer", { username: requested }),
  confirm: (instant: string) => t("dashpay.instant.confirm", { username: `@${instant}` }),
  success: (instant: string, requested: string) => t("dashpay.instant.success", { instant, requested }),
  failure: (instant: string) => t("dashpay.instant.failure", { username: instant }),
});

export const VOTE_COPY = withTable(messageTable("dashpay.vote", ["instantAction", "instantShort", "topUp"]), {
  instantHint: (requested: string) => t("dashpay.vote.instantHint", { requested }),
  wonMain: (username: string, instant: string) => t("dashpay.vote.wonMain", { username, instant }),
});

/** dpp's `convert_to_homograph_safe_chars`: lower case, `o` → `0`, `i` and `l` → `1`. */
export const homographSafe = (label: string) => label.toLowerCase().replace(/o/g, "0").replace(/[il]/g, "1");

// ---------------------------------------------------------------------------------------------
// How long a vote runs

/** Platform's contested-name vote poll, per network (rs-platform-version
 * `dpp_versions/dpp_voting_versions/v2.rs` lines 4-5 at PastaPastaPasta/platform c5917e6bd5, the
 * engine's pin): two weeks on mainnet, 90 minutes on every other network (rs-drive
 * `insert_contested/add_contested_document_for_contract_operations/v0/mod.rs` 55-68). */
export const VOTE_POLL_MS: Readonly<Partial<Record<NetworkKind, number>>> = {
  mainnet: 1_209_600_000,
  testnet: 5_400_000,
  devnet: 5_400_000,
  regtest: 5_400_000,
};

const WEEK_SECS = 7 * 86_400;

/** The vote's length on `network`, in words: whole weeks as weeks, else days, hours, minutes;
 * "about two weeks" for a network the table does not know. */
export function voteDuration(network: NetworkKind, polls: Readonly<Partial<Record<NetworkKind, number>>> = VOTE_POLL_MS): string {
  const ms = polls[network];
  if (ms === undefined) return t("dashpay.instant.aboutTwoWeeks");
  const secs = ms / 1000;
  if (secs % WEEK_SECS === 0) return t("dashpay.instant.weeks", { count: secs / WEEK_SECS });
  return spanLong(secs);
}

// ---------------------------------------------------------------------------------------------
// What became of a registration's temporary name (renderer-derived until the engine records it)

/** A names read: when it started (ms) and what it said, or `undefined` while none answered or the
 * last one failed. */
export interface NamesRead {
  startedAt: number;
  identities: IdentitySummary[];
}

/** `owned`: a names read says the identity owns the temporary name; `failed`: a read started
 * after the row last changed (so after it reached Contested, which is after the temporary name's
 * write) says it does not; `null`: nothing asked for, or no such read yet. A read in flight, one
 * started before the row's change, a failed one, or one whose names are not settled says nothing. */
export type TemporaryOutcome = "owned" | "failed" | null;

export function temporaryOutcome(row: RegistrationStatus | undefined, read: NamesRead | undefined): TemporaryOutcome {
  const temp = row?.temporaryLabel;
  if (!row || !temp || !row.identity || (row.phase !== "contested" && row.phase !== "done")) return null;
  const summary = read?.identities.find((s) => s.identity === row.identity);
  if (!read || !summary || summary.namesUpdating || summary.namesCheck !== null) return null;
  if (summary.names.some((n) => homographSafe(n) === homographSafe(temp))) return "owned";
  // Whole seconds: the row's time is the engine's (unix seconds, rounded down).
  return Math.floor(read.startedAt / 1000) > row.updatedAt ? "failed" : null;
}

/** The registration of the contest `label` by `identity` that asked for a temporary name. */
export function contestRow(rows: RegistrationStatus[] | undefined, identity: string, label: string): RegistrationStatus | undefined {
  return rows?.findLast(
    (r) => r.identity === identity && homographSafe(r.label) === homographSafe(label) && r.temporaryLabel !== null && (r.phase === "contested" || r.phase === "done"),
  );
}

/** The Join page's gate on registering at all: a wallet that holds its keys (the status's
 * `watchOnly` reason). Never the network: registration from the Dash wallet is live on mainnet. */
export function registrationOffered(wallet: { watchOnly: boolean } | undefined): boolean {
  return wallet !== undefined && !wallet.watchOnly;
}

export interface AskedTemporary {
  label: string;
  outcome: TemporaryOutcome;
}

/** What the voting card and the request's details offer while the own contest is open. */
export interface InstantInputs {
  /** DashPay registration is offered for this wallet: it holds its keys (the Join page's gate). */
  offered: boolean;
  /** Credits a plain name costs (`grant_request(registerName)`), `null` while unknown. */
  nameCredits: string | null;
  /** The temporary name the contest's registration asked for, and what became of it. */
  asked: AskedTemporary | null;
}

/** The card's instant offer for an open contest whose identity holds no other name. */
export type InstantOffer =
  /** Offer it: "Create instant username". */
  | { kind: "create" }
  /** The credits do not cover a name: "Top up". */
  | { kind: "short" }
  /** The registration's temporary name could not be registered: "Try again" with it. */
  | { kind: "failed"; label: string }
  /** Nothing to offer (gated off, names unsettled, a temporary name held or not known yet). */
  | null;

export function instantOffer(main: IdentitySummary | undefined, temporary: string | null, i: InstantInputs | undefined): InstantOffer {
  if (!i?.offered || !main || main.namesUpdating || main.namesCheck !== null || temporary) return null;
  if (i.asked) return i.asked.outcome === "failed" ? { kind: "failed", label: i.asked.label } : null;
  if (i.nameCredits !== null && main.balance !== null && BigInt(main.balance) < BigInt(i.nameCredits)) return { kind: "short" };
  return { kind: "create" };
}
