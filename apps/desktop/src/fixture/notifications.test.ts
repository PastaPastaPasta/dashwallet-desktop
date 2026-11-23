// The fixture's bell against the engine's: every case of vectors/notifications.json, which
// dw-engine records over a session (rust/crates/dw-engine/src/platform/notifications_tests.rs,
// `vectors`; CI fails when it is stale), replayed over the real IPC path. Regenerate with
// `DW_UPDATE_VECTORS=1 cargo test -p dw-engine notification_vectors`.
//
// The vector names identities: `me` is the wallet's identity, `dave` the sender whose request the
// cases ignore (here the world's first request, Grace's: not the world's Dave), `stranger` an
// identity the wallet lacks, `c<n>` other senders (`requests` gives the waiting ones' ids), and any
// other name is passed as is (an id that is not Base58). A time the engine stamped during the run
// is `"now"`.

import { clearMocks } from "@tauri-apps/api/mocks";
import { listen } from "@tauri-apps/api/event";
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { AppEvent } from "../api/generated/commands";
import type { DashPayEventInfo } from "../api/generated/DashPayEventInfo";
import type { DashPayEventPage } from "../api/generated/DashPayEventPage";
import { IDENTITY, JOURNAL_KINDS, type JournalKind } from "./dashpay";
import { installFixture, type FixtureHandle } from "./index";
import { FIXTURE_WALLET_ID } from "./scenarios";
import { FIXTURE_NOW, secs } from "./wallet";

type Now = number | "now";
interface Row {
  id: number;
  identity: string;
  kind: JournalKind;
  contact: string | null;
  ref: string;
  at: number;
  readAt: Now | null;
}
type Call =
  | { events: { identity: string; cursor: number | null; limit: number } }
  | { unreadCount: { identity: string } }
  | { markRead: { identity: string; upTo: number } }
  | { ignore: { contact: string } }
  | { unignore: { contact: string } }
  | { sweep: null }
  | { otherWallet: Call };
type Outcome = { ok: unknown } | { error: { code: string; data: unknown } };
interface Step {
  call: Call;
  result: Outcome | null;
}
interface Vectors {
  comment: string;
  notBase58: string;
  /** Base58 of an identity id's length that does not decode to 32 bytes. */
  wrongLength: string[];
  kinds: Record<string, string>;
  /** The requests waiting, as the engine was given them. */
  requests: { contact: string; id: string; since: number }[];
  rows: Row[];
  cases: { name: string; steps: Step[] }[];
}

const VECTORS: Vectors = JSON.parse(readFileSync(new URL("./vectors/notifications.json", import.meta.url), "utf8"));
const W = FIXTURE_WALLET_ID;
const OTHER_WALLET = "5a".repeat(32);
const NOW = secs(FIXTURE_NOW);

beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

const settle = (p: Promise<unknown>): Promise<Outcome> =>
  p.then(
    (ok) => ({ ok }),
    (e: { code: string; data: unknown }) => ({ error: { code: e.code, data: e.data } }),
  );

/** A world holding the vector's rows and requests: `me` is Alice, `dave` Grace. */
function seeded(fixture: FixtureHandle) {
  const dp = fixture.world.dashpay;
  const waiting = dp.contacts.filter((c) => c.summary.relation === "pendingIncoming");
  const dave = waiting[0];
  const ids = new Map<string, string>([
    ["me", IDENTITY.alice],
    ["dave", dave.summary.contact],
    ["stranger", IDENTITY.mallory],
  ]);
  // The vector's requests in place of the world's, in the reverse of the engine's order (oldest
  // first, ties by id descending), so only a sort shows them as the engine does.
  const requests = VECTORS.requests
    .map(({ contact, id, since }) => {
      if (contact !== "dave") ids.set(contact, id);
      const c = structuredClone(dave);
      c.summary = { ...c.summary, contact: ids.get(contact)!, username: contact, since };
      return c;
    })
    .sort((a, b) => a.summary.since! - b.summary.since! || (a.summary.contact < b.summary.contact ? 1 : -1));
  dp.contacts = [...dp.contacts.filter((c) => !waiting.includes(c)), ...requests];
  const id = (label: string) => ids.get(label) ?? (/^c\d+$/.test(label) ? `contact-${label}` : label);
  const label = (id: string) => [...ids].find(([, v]) => v === id)?.[0] ?? id.replace(/^contact-/, "");
  const time = (t: Now | null) => (t === "now" ? NOW : t);
  dp.journal = VECTORS.rows.map((r) => ({ ...r, identity: id(r.identity), contact: r.contact === null ? "" : id(r.contact), readAt: time(r.readAt) }));
  return { id, label };
}

async function replay(name: string, steps: Step[]) {
  const fixture = installFixture({ scenario: "dashpay-registered" });
  const { id, label } = seeded(fixture);
  const event = (e: DashPayEventInfo) => ({ ...e, contact: e.contact === null ? null : label(e.contact), readAt: e.readAt === NOW ? "now" : e.readAt });
  const shown = (o: Outcome): Outcome => {
    if (!("ok" in o) || o.ok === null || typeof o.ok !== "object") return o;
    const p = o.ok as DashPayEventPage;
    return { ok: { pending: p.pending.map((c) => label(c.contact)), new: p.new.map(event), earlier: p.earlier.map(event), nextCursor: p.nextCursor } };
  };
  const run = (call: Call, wallet: string): Promise<Outcome> | null => {
    if ("events" in call) return settle(cmd.dashpayEvents(wallet, id(call.events.identity), call.events.cursor, call.events.limit));
    if ("unreadCount" in call) return settle(cmd.dashpayUnreadCount(wallet, id(call.unreadCount.identity)));
    if ("markRead" in call) return settle(cmd.dashpayMarkRead(wallet, id(call.markRead.identity), call.markRead.upTo));
    if ("ignore" in call) return settle(cmd.dashpayIgnoreContact(wallet, IDENTITY.alice, id(call.ignore.contact)));
    if ("unignore" in call) return settle(cmd.dashpayUnignoreContact(wallet, IDENTITY.alice, id(call.unignore.contact)));
    // The fixture's world is past the next requests sweep already.
    if ("sweep" in call) return null;
    return run(call.otherWallet, OTHER_WALLET);
  };
  for (const [i, step] of steps.entries()) {
    const actual = run(step.call, W);
    expect(actual === null ? null : shown(await actual), `${name}: step ${i} ${JSON.stringify(step.call)}`).toEqual(step.result);
  }
}

describe("fixture bell matches the engine", () => {
  it("has the vectors", () => {
    expect(VECTORS.rows.length).toBeGreaterThan(200);
    expect(VECTORS.cases.map((c) => c.name)).toEqual(["pages", "markRead", "ignore", "refusals"]);
    const codes = VECTORS.cases.flatMap((c) => c.steps).flatMap((s) => (s.result && "error" in s.result ? [s.result.error.code] : []));
    expect(new Set(codes)).toEqual(new Set(["invalid_argument", "identity.not_found", "wallet_not_found"]));
    // Only decoding refuses these: each looks like an id.
    expect(VECTORS.wrongLength.length).toBeGreaterThan(0);
    for (const id of VECTORS.wrongLength) expect(id).toMatch(/^[1-9A-HJ-NP-Za-km-z]{32,44}$/);
  });

  it("takes every identity id of the fixture's worlds", async () => {
    installFixture({ scenario: "dashpay-registered" });
    for (const id of Object.values(IDENTITY)) expect(await cmd.dashpayUnreadCount(W, id), id).toBeGreaterThanOrEqual(0);
  });

  it("names the kinds as the engine", () => {
    expect(JOURNAL_KINDS).toEqual(VECTORS.kinds);
  });

  it.each(VECTORS.cases.map((c) => [c.name, c] as const))("%s", async (name, c) => {
    await replay(name, c.steps);
  });

  it("signals the feed only when a mark changed a row", async () => {
    const fixture = installFixture({ scenario: "dashpay-registered" });
    const signals: AppEvent[] = [];
    await listen<AppEvent>(cmd.ENGINE_EVENT, (e) => {
      if (e.payload.kind === "platform" && e.payload.change.domain === "events") signals.push(e.payload);
    });
    const me = IDENTITY.alice;
    const unread = await cmd.dashpayUnreadCount(W, me);
    expect(unread).toBeGreaterThan(0);
    await cmd.dashpayMarkRead(W, me, 0);
    await new Promise((r) => setTimeout(r, 0));
    expect(signals).toEqual([]);
    const page = await cmd.dashpayEvents(W, me, null, 200);
    await cmd.dashpayMarkRead(W, me, Math.max(...page.new.map((e) => e.id)));
    await new Promise((r) => setTimeout(r, 0));
    expect(signals).toEqual([{ kind: "platform", network: fixture.world.launch.network.dirName, walletId: W, change: { domain: "events", identity: me } }]);
    expect(await cmd.dashpayUnreadCount(W, me)).toBe(0);
  });
});

describe("the dashpay-registered bell", () => {
  it("shows the two requests waiting and the rows as the contacts have them", async () => {
    installFixture({ scenario: "dashpay-registered" });
    const page = await cmd.dashpayEvents(W, IDENTITY.alice, null, 50);
    // Newest first: Heidi's request came an hour after Grace's (DP2-07's fixture times).
    expect(page.pending.map((c) => c.contact)).toEqual([IDENTITY.heidi, IDENTITY.grace]);
    expect(page.new.map((e) => [e.kind, e.contact])).toEqual(
      expect.arrayContaining([
        ["requestReceived", IDENTITY.grace],
        ["requestReceived", IDENTITY.heidi],
        ["paymentReceived", IDENTITY.bob],
      ]),
    );
    expect(page.earlier.find((e) => e.kind === "usernameRegistered")?.reference).toBe("alice");
    expect(page.nextCursor).toBeNull();
    expect(await cmd.dashpayUnreadCount(W, IDENTITY.alice)).toBe(page.new.length);
  });
});
