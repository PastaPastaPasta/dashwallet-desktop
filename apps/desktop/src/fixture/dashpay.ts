// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// DashPay states for the fixture scenarios: no identity, a registration in flight, and a
// registered identity with contacts in every relation and channel state, pending requests in
// both directions, and a notification journal.

import type { ChannelStateName } from "../api/generated/ChannelStateName";
import type { ContactDetailInfo } from "../api/generated/ContactDetailInfo";
import type { ContactSectionName } from "../api/generated/ContactSectionName";
import type { DashPayProfile } from "../api/generated/DashPayProfile";
import type { DashPayStartupInfo } from "../api/generated/DashPayStartupInfo";
import type { DashPayStatus } from "../api/generated/DashPayStatus";
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
}

export interface RequestTimes {
  incoming: number | null;
  outgoing: number | null;
}

/** Base58 identity ids (44 characters, as Platform prints them). */
export const IDENTITY = {
  alice: "4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF",
  bob: "8GopLQQPjKr2gXjNEt9G8k2PzRZrdbBqQBNTCXV2DYiN",
  carol: "C3qVgrSTrGhfLD6qCZCa4Wm8CXYRUcuRHYiyYGNCqknj",
  dave: "DpPxRMjXzAv1hJDCFnRWGDSEZtqVvtxRrD9AAbGvVdsY",
  erin: "EZB2GDXKt8nP4ijLXAoMSYGXH3ixMiRDFyApvzqPnDPZ",
  frank: "FjhP3PdtkjBY9wzaLNVTmJD6fUjSHH8bFRxZVoSWkLrJ",
  grace: "GrAC3eQhZmKGwKu4R7PDuBkaxWDcXuspNwGM8RHm9ZsZ",
  heidi: "H31dSvQyJfALgo7SoD9w4uU2vYFXR3BpbYXr7FFTH2pa",
  ivan: "1vANuoMbBJYGLx5tPxaa5LSXmVSA7oHzJJvWmrHMKtpH",
  judy: "JudyGQjdXR1xmWvHFS3Kq8fQ5Jmpw3T6YVY6HAYtnBpW",
  mallory: "MALLoRyCRFtBn7r9fqD4yZcVaxu3Rx1oEkJ38EN6Hbnm",
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

export function registered(): DashPayWorld {
  const contacts = CONTACTS.map(contact);
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
    requestTimes: Object.fromEntries(contacts.map((c, i) => [c.summary.contact, requestTimes(c, i)])),
    directory: [
      ...contacts.map(hit),
      { identity: IDENTITY.mallory, username: "mallory", displayName: "Mallory", avatarUrl: null, relation: "stranger", unverified: false },
      { identity: IDENTITY.alice, username: "alice", displayName: "Alice Liddell", avatarUrl: null, relation: "isSelf", unverified: false },
    ],
  };
}
