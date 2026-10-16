// My Profile's view model (DASHPAY §4 `MyProfileViewModel`, `UsernameRequestStatusViewModel` and
// `CreditsViewModel`'s balance half; DP1-10): which state the page is in, the credits block with
// its warnings and its pending or stuck top-ups, the own contest's card, and the contest outcome
// rows. Plain TypeScript over the generated types; the top-up flow is topup.ts, the queries
// profileQueries.ts and the page screens/dashpay/MyProfile.tsx. Copy follows the mobile wallets
// (Android `strings-dashpay.xml`, the low-credit copy m4 item 44's), adapted (DP1-10 D-rows).

import type { ContestStatus } from "../api/generated/ContestStatus";
import type { CostTable } from "../api/generated/CostTable";
import type { CreditLevelName } from "../api/generated/CreditLevelName";
import type { DashPayEventInfo } from "../api/generated/DashPayEventInfo";
import type { DashPayEventPage } from "../api/generated/DashPayEventPage";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { PendingTopUp } from "../api/generated/PendingTopUp";
import type { StuckTopUpFunding } from "../api/generated/StuckTopUpFunding";
import { REG_COPY } from "./registration";
import { COPY, homeCard, namesState, type CardModel, type NamesState, type StatusInputs } from "./status";

export const PROFILE_COPY = {
  loading: "Loading your profile…",
  noProfile: "You have no DashPay profile yet",
  editProfile: "Edit profile",
  editLater: "Editing your profile comes with a later version.",
  usernames: "Usernames",
  identity: "Identity",
  noUsername: "No username",
  /** The requested name while its vote runs and no other name is held (as the chip says it). */
  contested: (label: string) => `@${label} · Pending`,
  updating: "Updating…",
  notLoaded: "Username not loaded",
  chooseName: "Choose a username",
  credits: {
    title: "Credit balance",
    unknown: "Credit balance unknown",
    unknownLine: "Dash Wallet reads it from the network.",
    updating: "Updating…",
    /** m3: the balance while the cost table, which turns it into Dash, is not read. */
    creditsOnly: (credits: string) => `${credits} credits`,
    estimate: (requests: string, updates: string) => `≈ ${requests} contact requests · ${updates} profile updates`,
    low: {
      title: "Your credit balance is low",
      text: "Top-up your credits to continue making changes to your profile and adding contacts",
    },
    empty: {
      title: "Your credit balance is fully depleted",
      text: "You can continue to use DashPay for payments but you cannot update your profile or add more contacts until you top up your credit balance",
    },
    topUp: "Top up",
    toppingUp: "Topping up…",
    pendingFirst: "Finish the pending top-up first.",
    /** D14: a top-up that ended while the sheet was hidden. */
    unseen: "Your top-up has an update.",
    showResult: "Show",
  },
  pending: {
    title: (amount: string) => `Top-up of ${amount} pending`,
    unproved: "Waiting for the network to confirm the payment.",
    proved: "Ready to add.",
    unknown: "The network says this payment was already used. Check your balance.",
    finish: "Finish",
    arrived: "Credits arrived",
  },
  stuck: {
    maybeSent: COPY.repair.unknown,
    willBeSent: "The payment is being sent",
    text: "Whether this top-up's payment reached the network is unknown. Tools ▸ Repair settles it; you can top up again once it has.",
    action: COPY.repair.action,
  },
  request: {
    title: COPY.voting.title,
    deciding: COPY.voting.deciding,
    endsOn: COPY.voting.endsOn,
    notify: COPY.voting.notify,
    temporary: COPY.voting.temporary,
    instant: REG_COPY.name.instantTitle,
    instantLine: "Register a second username now and use it until the vote ends.",
    won: { title: (label: string) => `@${label} is yours`, text: "The network approved your username." },
    lost: (label: string) => `@${label} went to another user`,
    lostNoWinner: (label: string) => `Voting on @${label} ended with no winner`,
    locked: { title: (label: string) => `@${label} was blocked`, text: "The network voted to block this username." },
    readFailed: "Could not read the votes.",
    retry: "Try again",
    votingLink: "What is username voting?",
    votesFor: "Votes for you",
    others: "Other requests",
    othersValue: (k: number, votes: string) => `${k}, ${votes} votes`,
    block: "Votes to block",
    abstain: "Abstained",
  },
  settings: { joined: "Your DashPay identity", notJoined: "Not joined", profile: "My Profile", join: "Join DashPay" },
  outcomes: {
    title: "Username voting results",
    won: (label: string) => `@${label} — yours`,
    lost: (label: string) => `@${label} — went to another user`,
    locked: (label: string) => `@${label} — blocked`,
  },
} as const;

/** The own contest the status reports while one runs. */
export interface ContestRef {
  label: string;
  endsAt: number | null;
}

/** The page's state (P1–P6); `data-state` on `profile-body`. */
export type PageModel =
  | { kind: "loading" }
  /** P2, P3: DP1-08's card for the state (D2); `noProfile` says the heading over it. */
  | { kind: "card"; card: CardModel; noProfile: boolean }
  /** P4–P6: the main identity's page; `contest` is the status's contest while one runs. */
  | { kind: "ready"; main: IdentitySummary; contest: ContestRef | null };

export function pageModel(i: StatusInputs): PageModel {
  if (i.statusError) return { kind: "card", card: homeCard(i)!, noProfile: false };
  const s = i.status;
  if (s && (s.kind === "ready" || s.kind === "contestPending")) {
    if (!i.main) return { kind: "loading" };
    return { kind: "ready", main: i.main, contest: s.kind === "contestPending" ? { label: s.label, endsAt: s.endsAt } : null };
  }
  const c = s ? homeCard(i) : null;
  return c ? { kind: "card", card: c, noProfile: true } : { kind: "loading" };
}

/** The header (D4): the display name, else the username; @name or what the names state says
 * (the contested label while its vote runs and the identity holds no name). */
export interface HeaderModel {
  title: string;
  /** "@name", or the names state's text. */
  username: string;
  names: NamesState;
  message: string | null;
  /** The initials avatar's letters. */
  initials: string;
}

export function headerModel(main: IdentitySummary, contest: string | null = null): HeaderModel {
  const names = namesState(main);
  const none = contest === null ? PROFILE_COPY.noUsername : PROFILE_COPY.contested(contest);
  const username = names === "named" ? `@${main.mainName}` : names === "none" ? none : names === "unknown" ? PROFILE_COPY.notLoaded : PROFILE_COPY.updating;
  const display = main.profile?.displayName?.trim() || null;
  const title = display ?? (main.mainName || username);
  const initials = (Array.from(display ?? main.mainName ?? "?")[0] ?? "?").toUpperCase();
  return { title, username, names, message: main.profile?.publicMessage?.trim() || null, initials };
}

/** Credits as duffs (D6): what they would buy back, rounded down. */
export const creditsToDuffs = (credits: string | bigint, costs: Pick<CostTable, "creditsPerDuff">) => Number(BigInt(credits) / BigInt(costs.creditsPerDuff));

/** Android's `buy_credits_estimated_items` (D6): how many contact requests and profile updates
 * `credits` pay for. */
export function estimateText(credits: string | bigint, costs: CostTable): string {
  const c = BigInt(credits);
  const n = (cost: string) => (BigInt(cost) > 0n ? (c / BigInt(cost)).toLocaleString("en-US") : "—");
  return PROFILE_COPY.credits.estimate(n(costs.contactRequest), n(costs.profileUpdate));
}

export interface PendingRow {
  txid: string;
  title: string;
  line: string;
  /** "Credits arrived" (`resolve_top_up`): a proved lock only (D15). */
  arrived: boolean;
}

export interface StuckRow {
  step: string;
  title: string;
  text: string;
}

export interface CreditsModel {
  /** `data-level`: the engine's level, `unknown` while the balance is. */
  level: CreditLevelName | "unknown";
  /** The balance in Dash (in credits while the cost table is not read), masked in discreet mode
   * (D7); `null` while unknown. */
  figure: string | null;
  estimate: string | null;
  /** C5: the balance may be behind (D9). */
  updating: boolean;
  warning: { title: string; text: string } | null;
  pending: PendingRow[];
  stuck: StuckRow[];
  topUp: { disabled: boolean; reason: string | null; primary: boolean };
  /** D14, D31: the sheet holds a result for this identity not yet seen. */
  unseen: boolean;
}

export interface CreditsInputs {
  costs: CostTable | undefined;
  /** The wallet's pending top-ups and stuck fundings; the identity's are picked out. */
  pending: PendingTopUp[];
  stuck: StuckTopUpFunding[];
  /** This wallet's top-up runs now (D14). */
  toppingUp: boolean;
  /** The flow holds a result for this identity that came while the sheet was hidden (D14, D31). */
  unseen: boolean;
  /** Locks a call answered `consumptionUnknown` this session (D15). */
  answeredUnknown: ReadonlySet<string>;
  amount: (duffs: number) => string;
  /** The display's mask for a hidden amount. */
  masked: string;
  discreet: boolean;
}

/** A row of the wallet that belongs to `main`: by its identity id, else by its index. */
const ofIdentity = (main: IdentitySummary) => (r: { identity: string | null; identityIndex: number }) =>
  r.identity === main.identity || (r.identity === null && r.identityIndex === main.index);

/** A pending lock or a stuck funding of `main` refuses its next top-up (m4 §2.8): Top up waits. */
export const holdsTopUp = (main: IdentitySummary, pending: PendingTopUp[], stuck: StuckTopUpFunding[]) =>
  pending.some(ofIdentity(main)) || stuck.some(ofIdentity(main));

/** A credit balance as shown (D29, D30): the amount once the cost table is read, raw credits until then; masked only in Discreet Mode. */
export function creditBalance(credits: string, costs: CostTable | undefined, discreet: boolean, amount: (duffs: number) => string, masked: string): string {
  if (discreet) return masked;
  return costs ? amount(creditsToDuffs(credits, costs)) : PROFILE_COPY.credits.creditsOnly(BigInt(credits).toLocaleString("en-US"));
}

/** The credits block (C1–C8). */
export function creditsModel(main: IdentitySummary, i: CreditsInputs): CreditsModel {
  const known = main.balance !== null;
  const level = main.creditLevel ?? (known ? "ok" : "unknown");
  let figure: string | null = null;
  let estimate: string | null = null;
  if (main.balance !== null) {
    figure = creditBalance(main.balance, i.costs, i.discreet, i.amount, i.masked);
    if (i.costs) estimate = i.discreet ? PROFILE_COPY.credits.estimate(i.masked, i.masked) : estimateText(main.balance, i.costs);
  }
  const warning = level === "low" ? PROFILE_COPY.credits.low : level === "empty" ? PROFILE_COPY.credits.empty : null;
  const pending = i.pending
    .filter(ofIdentity(main))
    .map((p): PendingRow => {
      // Platform said the lock was used: the row's flag, or a call's answer this session (D15).
      const unknown = p.proved && (p.consumptionUnknown || i.answeredUnknown.has(p.txid));
      return {
        txid: p.txid,
        title: PROFILE_COPY.pending.title(i.amount(p.duffs)),
        line: unknown ? PROFILE_COPY.pending.unknown : p.proved ? PROFILE_COPY.pending.proved : PROFILE_COPY.pending.unproved,
        arrived: unknown,
      };
    });
  const stuck = i.stuck
    .filter(ofIdentity(main))
    .map((s): StuckRow => ({ step: s.step, title: s.state === "willBeSent" ? PROFILE_COPY.stuck.willBeSent : PROFILE_COPY.stuck.maybeSent, text: PROFILE_COPY.stuck.text }));
  // A stuck funding or a pending lock refuses the next top-up (m4 §2.8): say so before Confirm.
  const reason = stuck.length > 0 ? stuck[0].title : pending.length > 0 ? PROFILE_COPY.credits.pendingFirst : i.toppingUp ? PROFILE_COPY.credits.toppingUp : null;
  return {
    level,
    figure,
    estimate,
    updating: known && main.balanceStale,
    warning,
    pending,
    stuck,
    topUp: { disabled: reason !== null, reason, primary: warning !== null },
    unseen: i.unseen,
  };
}

// ---------------------------------------------------------------------------------------------
// The request card (R1–R6)

export interface TallyRow {
  label: string;
  value: string;
}

export interface RequestModel {
  /** `data-state`: `open` while the vote runs, else its result. */
  state: "open" | "won" | "lost" | "locked";
  title: string;
  lines: string[];
  /** D11; `null` when the votes are not known (unread or failed). */
  tally: TallyRow[] | null;
  /** The temporary name while the vote runs (DASHPAY F5); `null`: none, offer an instant one. */
  temporary: string | null;
  /** Offer "Create instant username" (D13). */
  instant: boolean;
  /** R6: `contest_status` failed. */
  readFailed: boolean;
}

const votes = (n: number | null) => (n === null ? "—" : n.toLocaleString("en-US"));

/** D11: the tally; the other contenders summed, as none of them owns a username yet. */
export function tallyRows(c: ContestStatus): TallyRow[] {
  const self = c.contenders.find((x) => x.isSelf);
  const others = c.contenders.filter((x) => !x.isSelf);
  const otherVotes = others.some((x) => x.votes === null) ? null : others.reduce((n, x) => n + (x.votes ?? 0), 0);
  const C = PROFILE_COPY.request;
  return [
    { label: C.votesFor, value: votes(self?.votes ?? null) },
    { label: C.others, value: C.othersValue(others.length, votes(otherVotes)) },
    { label: C.block, value: votes(c.lockVotes) },
    { label: C.abstain, value: votes(c.abstainVotes) },
  ];
}

/** The own contest's card while the status is `contestPending` (R1–R6). The temporary name is
 * the contest's own when it reads one, else the main name when it differs from the label (as
 * the Home card says it, DP1-08). */
export function requestModel(
  label: string,
  endsAt: number | null,
  contest: ContestStatus | undefined,
  readFailed: boolean,
  main: IdentitySummary,
  date: (secs: number) => string,
): RequestModel {
  const C = PROFILE_COPY.request;
  const read = contest && contest.label === label ? contest : undefined;
  switch (read?.state.kind) {
    case "won":
      return { state: "won", title: C.won.title(label), lines: [C.won.text], tally: tallyRows(read), temporary: null, instant: false, readFailed: false };
    case "lost":
      return {
        state: "lost",
        title: read.state.winner === null ? C.lostNoWinner(label) : C.lost(label),
        lines: [],
        tally: tallyRows(read),
        temporary: null,
        instant: false,
        readFailed: false,
      };
    case "locked":
      return { state: "locked", title: C.locked.title(label), lines: [C.locked.text], tally: tallyRows(read), temporary: null, instant: false, readFailed: false };
    default: {
      const end = read?.endsAt ?? endsAt;
      const fromMain = !main.namesUpdating && main.mainName && main.mainName !== label ? main.mainName : null;
      const temporary = read?.temporaryName ?? fromMain;
      const lines: string[] = [C.deciding, end === null ? C.notify : C.endsOn(date(end))];
      if (temporary) lines.push(C.temporary(temporary));
      return { state: "open", title: C.title(label), lines, tally: read ? tallyRows(read) : null, temporary, instant: !temporary, readFailed: readFailed && !read };
    }
  }
}

// ---------------------------------------------------------------------------------------------
// Outcome rows (D12)

export interface OutcomeRow {
  id: number;
  text: string;
  /** UNIX seconds. */
  at: number;
}

/** How many outcome rows the page keeps (D12). */
const OUTCOME_ROWS = 3;

/** The newest contest outcomes on the bell's first page (D12). */
export function outcomeRows(page: DashPayEventPage | undefined): OutcomeRow[] {
  if (!page) return [];
  const text = (e: DashPayEventInfo): string | null => {
    const label = e.reference ?? "";
    switch (e.kind) {
      case "contestWon":
        return PROFILE_COPY.outcomes.won(label);
      case "contestLost":
        return PROFILE_COPY.outcomes.lost(label);
      case "contestLocked":
        return PROFILE_COPY.outcomes.locked(label);
      default:
        return null;
    }
  };
  return [...page.new, ...page.earlier]
    .sort((a, b) => b.at - a.at || b.id - a.id)
    .flatMap((e) => {
      const t = text(e);
      return t === null ? [] : [{ id: e.id, text: t, at: e.at }];
    })
    .slice(0, OUTCOME_ROWS);
}
