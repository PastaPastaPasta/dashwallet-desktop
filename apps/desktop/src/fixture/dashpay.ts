// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// DashPay states for the fixture scenarios: no identity, a registration in flight, and a
// registered identity with contacts in every relation and channel state, pending requests in
// both directions, and a notification journal.

import type { ChannelStateName } from "../api/generated/ChannelStateName";
import type { ContactDetailInfo } from "../api/generated/ContactDetailInfo";
import type { ContactSectionName } from "../api/generated/ContactSectionName";
import type { DashPayProfile } from "../api/generated/DashPayProfile";
import type { DashPayStartupInfo } from "../api/generated/DashPayStartupInfo";
import type { DashPayEventInfo } from "../api/generated/DashPayEventInfo";
import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { EventKindName } from "../api/generated/EventKindName";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { UserHit } from "../api/generated/UserHit";
import type { UserRelation } from "../api/generated/UserRelation";
import { FIXTURE_NOW, secs } from "./wallet";

export interface DashPayWorld {
  status: DashPayStatus;
  startup: DashPayStartupInfo;
  identities: IdentitySummary[];
  registrations: RegistrationStatus[];
  /** The main identity's contacts, every relation but `stranger` and `isSelf`. */
  contacts: ContactDetailInfo[];
  /** What `search_users` / `resolve_user` find: the contacts plus a stranger and the wallet's own name. */
  directory: UserHit[];
  /** Each contact's requests by contact id: their Platform `$createdAt` in ms, theirs to the
   * main identity and its own to them (the engine's journal `ref` of a request, journal.rs). */
  requestTimes: Record<string, RequestTimes>;
  /** The `dp_events` journal of the wallet's identities, oldest first: the bell's rows. */
  journal: JournalRow[];
}

/** A journal row as dw-appdb stores it: the kind's journal name, `""` for no contact or reference,
 * and a name kind's reference with its `@<stamp>`. */
export interface JournalRow {
  id: number;
  identity: string;
  kind: JournalKind;
  contact: string;
  ref: string;
  at: number;
  readAt: number | null;
}

/** The journal's kind names and dw-app's `EventKindName` for each (vectors/notifications.json `kinds`). */
export const JOURNAL_KINDS = {
  username_registered: "usernameRegistered",
  contest_won: "contestWon",
  contest_lost: "contestLost",
  contest_locked: "contestLocked",
  request_received: "requestReceived",
  request_accepted: "requestAccepted",
  contact_established: "contactEstablished",
  payment_received: "paymentReceived",
} as const satisfies Record<string, EventKindName>;
export type JournalKind = keyof typeof JOURNAL_KINDS;

/** The event a row shows (notifications.rs `DashPayEvent::journaled`): no empty contact or
 * reference, and a name kind's label without its `@<stamp>`. */
export function eventOf(row: JournalRow): DashPayEventInfo {
  const named = row.kind === "username_registered" || row.kind.startsWith("contest_");
  const reference = named ? row.ref.split("@")[0] : row.ref;
  return {
    id: row.id,
    kind: JOURNAL_KINDS[row.kind],
    contact: row.contact || null,
    reference: reference || null,
    at: row.at,
    readAt: row.readAt,
  };
}

export interface RequestTimes {
  incoming: number | null;
  outgoing: number | null;
}

/** The engine's `main_username`: the main identity's main name, none while its names update. */
export function mainUsername(d: DashPayWorld): string | null {
  const main = d.identities.find((i) => i.isMain);
  return main && !main.namesUpdating ? main.mainName : null;
}

/** Base58 identity ids: 32 bytes, 43 or 44 characters as Platform prints them. */
export const IDENTITY = {
  alice: "4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF",
  bob: "8GopLQQPjKr2gXjNEt9G8k2PzRZrdbBqQBNTCXV2DYiN",
  carol: "C3qVgrSTrGhfLD6qCZCa4Wm8CXYRUcuRHYiyYGNCqknj",
  dave: "DpPxRMjXzAv1hJDCFnRWGDSEZtqVvtxRrD9AAbGvVdsY",
  erin: "EZB2GDXKt8nP4ijLXAoMSYGXH3ixMiRDFyApvzqPnDPZ",
  frank: "FjhP3PdtkjBY9wzaLNVTmJD6fUjSHH8bFRxZVoSWkLrJ",
  grace: "GrAC3eQhZmKGwKu4R7PDuBkaxWDcXuspNwGM8RHm9ZsZ",
  heidi: "H31dSvQyJfALgo7SoD9w4uU2vYFXR3BpbYXr7FFTH2pa",
  ivan: "1vANuoMbBJYGLx5tPxaaLSXmVSA7oHzJJvWmrHMKtpH",
  judy: "JudyGQjdXR1xmWvHFS3K8fQ5Jmpw3T6YVY6HAYtnBpW",
  mallory: "MALLoRyCRFtBn7r9fqD4ZcVaxu3Rx1oEkJ38EN6Hbnm",
} as const;

const at = (daysAgo: number, hours = 0) => secs(FIXTURE_NOW) - daysAgo * 86_400 - hours * 3_600;

const profile = (displayName: string | null, message: string | null = null): DashPayProfile => ({
  displayName,
  publicMessage: message,
  avatarUrl: null,
  avatarHash: null,
  avatarFingerprint: null,
  updatedAt: at(30),
});

interface ContactSpec {
  name: keyof typeof IDENTITY;
  displayName: string | null;
  relation: Exclude<UserRelation, "stranger" | "isSelf">;
  channel: ChannelStateName;
  since: number | null;
  lastActivityAt?: number | null;
  alias?: string | null;
  note?: string | null;
  hidden?: boolean;
  paymentLockTxid?: string;
  unverified?: boolean;
}

/** Every relation and every channel state at least once. */
const CONTACTS: ContactSpec[] = [
  { name: "bob", displayName: "Bob Builder", relation: "contact", channel: "ready", since: at(40), lastActivityAt: at(1, 3) },
  { name: "carol", displayName: "Carol", relation: "contact", channel: "setupPending", since: at(2) },
  {
    name: "dave",
    displayName: null,
    relation: "contact",
    channel: "broken",
    since: at(90),
    lastActivityAt: at(12),
    paymentLockTxid: "d4".repeat(32),
  },
  { name: "erin", displayName: "Erin", relation: "contact", channel: "notEstablished", since: at(5), unverified: true },
  { name: "frank", displayName: "Frank", relation: "pendingOutgoing", channel: "notEstablished", since: null },
  { name: "grace", displayName: "Grace Hopper", relation: "pendingIncoming", channel: "notEstablished", since: null },
  { name: "heidi", displayName: null, relation: "pendingIncoming", channel: "notEstablished", since: null },
  { name: "ivan", displayName: "Ivan", relation: "ignored", channel: "notEstablished", since: null },
  {
    name: "judy",
    displayName: "Judy",
    relation: "contact",
    channel: "ready",
    since: at(200),
    lastActivityAt: at(60),
    alias: "Judy (work)",
    note: "Pays the invoices",
    hidden: true,
  },
];

function contact(spec: ContactSpec): ContactDetailInfo {
  const pending = spec.relation === "pendingIncoming" || spec.relation === "pendingOutgoing";
  return {
    summary: {
      contact: IDENTITY[spec.name],
      username: spec.name,
      displayName: spec.displayName,
      alias: spec.alias ?? null,
      relation: spec.relation,
      hidden: spec.hidden ?? false,
      channel: spec.channel,
      since: spec.since,
      lastActivityAt: spec.lastActivityAt ?? null,
      unverified: spec.unverified ?? false,
    },
    profile: spec.displayName ? profile(spec.displayName) : null,
    privateDetails: { alias: spec.alias ?? null, note: spec.note ?? null, hidden: spec.hidden ?? false },
    publishState: spec.alias || spec.note ? "published" : "local",
    requestSentAt: spec.relation === "pendingIncoming" ? null : (spec.since ?? (pending ? at(1) : null)),
    requestReceivedAt: spec.relation === "pendingOutgoing" ? null : (spec.since ?? (pending ? at(0, 2) : null)),
    paymentLock: spec.paymentLockTxid ? { txid: spec.paymentLockTxid, since: at(0, 5) } : null,
  };
}

/** A request's `$createdAt` (ms) in the second its contact shows, with a millisecond part. */
const ms = (at: number | null, i: number) => (at === null ? null : at * 1000 + ((i * 137 + 41) % 1000));

function requestTimes(c: ContactDetailInfo, i: number): RequestTimes {
  return { incoming: ms(c.requestReceivedAt, i), outgoing: ms(c.requestSentAt, i + 3) };
}

/** The section `dashpay_contacts` lists a contact in. */
export function sectionOf(c: ContactDetailInfo): ContactSectionName {
  const { relation, hidden } = c.summary;
  if (relation === "pendingIncoming") return "requests";
  if (relation === "pendingOutgoing") return "pending";
  return hidden || relation === "ignored" ? "hidden" : "contacts";
}

function aliceIdentity(): IdentitySummary {
  return {
    identity: IDENTITY.alice,
    index: 0,
    names: ["alice"],
    mainName: "alice",
    namesUpdating: false,
    isMain: true,
    balance: "184500000000",
    // Refreshed a minute before FIXTURE_NOW, so not stale (the engine's ten minutes); far above
    // the cost table's low threshold (ten of its dearest action), so `ok`.
    balanceAsOf: FIXTURE_NOW.getTime() - 60_000,
    balanceStale: false,
    hasDashpayKeys: true,
    profile: profile("Alice Liddell", "Down the rabbit hole"),
    unverified: false,
    creditLevel: "ok",
  };
}

const startup = (s: DashPayStartupInfo["startup"], identity: string | null): DashPayStartupInfo => ({
  startup: s,
  readOnly: false,
  identity,
  contactAccountsPending: 0,
  identityScanIncomplete: false,
  finishedAt: at(0, 1),
});

export function noIdentity(): DashPayWorld {
  return {
    status: { kind: "noIdentity", reason: null },
    startup: startup("noIdentity", null),
    identities: [],
    registrations: [],
    contacts: [],
    directory: [],
    requestTimes: {},
    journal: [],
  };
}

/** A registration of "alice" funded and waiting for its InstantSend lock. */
export function registering(): DashPayWorld {
  return {
    ...noIdentity(),
    status: { kind: "registering", draft: "reg-1" },
    registrations: [
      {
        draft: "reg-1",
        phase: "fundingSent",
        label: "alice",
        temporaryLabel: null,
        identity: null,
        txid: "f0".repeat(32),
        waiting: "instantSend",
        holdsKey: true,
        fundsCommitted: true,
        contestEndsAt: null,
        failure: null,
        nameFailure: null,
        profileFailure: null,
        createdAt: at(0, 1),
        updatedAt: at(0, 0),
      },
    ],
  };
}

/** Alice's bell: her name, a lost contest, each contact's accept as the contacts show it, the
 * requests waiting, and payments; what came before the last day is read. */
function journal(contacts: ContactDetailInfo[], times: Record<string, RequestTimes>): JournalRow[] {
  type Seed = Omit<JournalRow, "id" | "identity">;
  const seeds: Seed[] = [
    { kind: "username_registered", contact: "", ref: `alice@${at(120)}`, at: at(120), readAt: at(119) },
    { kind: "contest_lost", contact: "", ref: `al@${at(118)}`, at: at(118), readAt: at(117) },
  ];
  for (const c of contacts) {
    const id = c.summary.contact;
    const { incoming, outgoing } = times[id];
    const seen = (t: number) => (t < at(1) ? t + 600 : null);
    const { relation } = c.summary;
    if (relation === "contact" && c.requestReceivedAt !== null && incoming !== null) {
      // Theirs came first: our accept made the contact. Ours first: theirs accepted it.
      const ours = outgoing !== null && outgoing < incoming;
      const t = c.requestReceivedAt;
      seeds.push(
        ours
          ? { kind: "request_accepted", contact: id, ref: String(incoming), at: t, readAt: seen(t) }
          : { kind: "contact_established", contact: id, ref: String(outgoing ?? incoming), at: t, readAt: seen(t) },
      );
    }
    if (relation === "pendingIncoming" && incoming !== null) {
      seeds.push({ kind: "request_received", contact: id, ref: String(incoming), at: Math.floor(incoming / 1000), readAt: null });
    }
  }
  seeds.push(
    { kind: "payment_received", contact: IDENTITY.bob, ref: "b0".repeat(32), at: at(1, 3), readAt: null },
    { kind: "payment_received", contact: IDENTITY.dave, ref: "d4".repeat(32), at: at(12), readAt: at(11) },
  );
  return seeds
    .sort((a, b) => a.at - b.at)
    .map((s, i) => ({ id: i + 1, identity: IDENTITY.alice, ...s }));
}

export function registered(): DashPayWorld {
  const contacts = CONTACTS.map(contact);
  const requestTimesById = Object.fromEntries(contacts.map((c, i) => [c.summary.contact, requestTimes(c, i)]));
  const hit = (c: ContactDetailInfo): UserHit => ({
    identity: c.summary.contact,
    username: c.summary.username!,
    displayName: c.summary.displayName,
    avatarUrl: null,
    relation: c.summary.relation,
    unverified: c.summary.unverified,
  });
  return {
    status: { kind: "ready", main: IDENTITY.alice },
    startup: startup("ready", IDENTITY.alice),
    identities: [aliceIdentity()],
    registrations: [],
    contacts,
    requestTimes: requestTimesById,
    journal: journal(contacts, requestTimesById),
    directory: [
      ...contacts.map(hit),
      { identity: IDENTITY.mallory, username: "mallory", displayName: "Mallory", avatarUrl: null, relation: "stranger", unverified: false },
      { identity: IDENTITY.alice, username: "alice", displayName: "Alice Liddell", avatarUrl: null, relation: "isSelf", unverified: false },
    ],
  };
}
