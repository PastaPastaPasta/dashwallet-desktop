// The contract vector of `dashpay_accept_request`'s grant rules (Sol r1 #3): dw-engine records
// what the engine does in vectors/accept.json (requests_tests.rs `accept_grant_vectors`, which
// fails when the file drifts); this replays each case through the fixture backend over the real
// IPC path and asserts the same outcome, step by step.

import { clearMocks } from "@tauri-apps/api/mocks";
import { listen } from "@tauri-apps/api/event";
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { AppEvent } from "../api/generated/commands";
import type { FlowKindName } from "../api/generated/FlowKindName";
import type { GrantPurposeSpec } from "../api/generated/GrantPurposeSpec";
import type { Lease } from "../api/generated/Lease";
import { IDENTITY } from "./dashpay";
import { installFixture, type FixtureHandle } from "./index";
import { FIXTURE_WALLET_ID } from "./scenarios";

type Ref = number | "" | "unknownVault" | "unknownLease";
type Party = "incoming" | "self" | "contact" | "stranger";
type Step =
  | { authorize: { purpose: "platformOp"; maxCredits: string; wallet: "own" | "other" } | { purpose: "spend"; maxDuffs: number; wallet: "own" | "other" } }
  | { beginFlow: { flow: FlowKindName; grants: Ref[] } }
  | { endFlow: number }
  | { accept: { from: Party; grant: Ref } }
  | { credits: number }
  | { lock: null }
  | { unlock: null }
  | { leases: null }
  | { leaseEvents: null };
type Outcome = { ok: unknown } | { error: { code: string; data: unknown } };
interface Case {
  name: string;
  balance?: string;
  dashpayKeys?: boolean;
  steps: Step[];
  results: Outcome[];
}
interface Vectors {
  comment: string;
  quote: string;
  unknownVault: string;
  unknownLease: string;
  otherWallet: string;
  request: { rule: string; incomingCreatedAt: string; acceptCreatedAt: string };
  cases: Case[];
}

const VECTORS: Vectors = JSON.parse(readFileSync(new URL("./vectors/accept.json", import.meta.url), "utf8"));
const W = FIXTURE_WALLET_ID;
const ME = IDENTITY.alice;

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

/** The fixture world's stand-ins for the engine fixture's FRANK, ME, ERIN and BOB. */
function parties(fixture: FixtureHandle): Record<Party, string> {
  const contacts = fixture.world.dashpay.contacts;
  const incoming = contacts.find((c) => c.summary.relation === "pendingIncoming")!.summary.contact;
  const contact = contacts.find((c) => c.summary.relation === "contact")!.summary.contact;
  const stranger = fixture.world.dashpay.directory.find((u) => u.relation === "stranger")!.identity;
  return { incoming, self: ME, contact, stranger };
}

/** A lease as the vector records it: less what depends on the run. */
const leaseShape = ({ id: _id, walletId: _w, keyExpiresInSecs: _k, ...rest }: Lease) => rest;

/** JSON with sorted keys: the order the vector lists leases in (serde_json's `Value::to_string`). */
const canonical = (v: unknown): string =>
  v !== null && typeof v === "object" && !Array.isArray(v)
    ? `{${Object.keys(v)
        .sort()
        .map((k) => `${JSON.stringify(k)}:${canonical((v as Record<string, unknown>)[k])}`)
        .join(",")}}`
    : Array.isArray(v)
      ? `[${v.map(canonical).join(",")}]`
      : JSON.stringify(v);

const ok = (o: Outcome): Outcome => ("ok" in o ? { ok: null } : o);

async function replay(c: Case) {
  clearMocks();
  const fixture = installFixture({ scenario: "dashpay-registered" });
  const before = new Set(fixture.world.leases.map((l) => l.id));
  const events: Lease[] = [];
  await listen<AppEvent>(cmd.ENGINE_EVENT, (e) => {
    if (e.payload.kind === "leaseChanged" && !before.has(e.payload.lease.id)) events.push(e.payload.lease);
  });
  let eventsSeen = 0;
  const me = fixture.world.dashpay.identities.find((i) => i.identity === ME)!;
  if (c.balance !== undefined) me.balance = c.balance;
  if (c.dashpayKeys === false) me.hasDashpayKeys = false;
  const who = parties(fixture);
  const credential = { kind: "passphrase", passphrase: fixture.world.passphrase } as const;
  const ids: (string | null)[] = [];
  const ref = (r: Ref) => (typeof r === "number" ? ids[r]! : r === "unknownVault" ? VECTORS.unknownVault : r === "unknownLease" ? VECTORS.unknownLease : r);

  for (const [i, step] of c.steps.entries()) {
    const at = `${c.name}: step ${i} ${JSON.stringify(step)}`;
    const expected = c.results[i];
    let actual: Outcome;
    let incomingBefore: number | null = null;
    let from: string | null = null;
    if ("authorize" in step) {
      const a = step.authorize;
      const purpose: GrantPurposeSpec =
        a.purpose === "platformOp" ? { kind: "platformOp", maxDuffs: 0, maxCredits: a.maxCredits } : { kind: "spend", maxDuffs: a.maxDuffs };
      actual = await settle(cmd.authorize(purpose, a.wallet === "own" ? W : VECTORS.otherWallet, credential));
    } else if ("beginFlow" in step) {
      actual = await settle(cmd.beginFlow(W, step.beginFlow.flow, step.beginFlow.grants.map(ref)));
    } else if ("endFlow" in step) {
      actual = await settle(cmd.endFlow(ref(step.endFlow)));
    } else if ("accept" in step) {
      from = who[step.accept.from];
      incomingBefore = fixture.world.dashpay.requestTimes[from]?.incoming ?? null;
      actual = await settle(cmd.dashpayAcceptRequest(W, ME, from, ref(step.accept.grant)));
    } else if ("lock" in step) {
      actual = ok(await settle(cmd.lock()));
    } else if ("unlock" in step) {
      actual = ok(await settle(cmd.unlock(fixture.world.passphrase)));
    } else if ("leases" in step) {
      const leases = (await cmd.leases()).filter((l) => !before.has(l.id)).map(leaseShape);
      actual = { ok: leases.sort((a, b) => (canonical(a) < canonical(b) ? -1 : 1)) };
    } else if ("leaseEvents" in step) {
      await new Promise((r) => setTimeout(r, 0));
      actual = { ok: events.slice(eventsSeen).map(leaseShape) };
      eventsSeen = events.length;
    } else {
      const id = ref(step.credits);
      const lease = (await cmd.leases()).find((l) => l.id === id.slice("lease-".length, "lease-".length + 8))!;
      const credits = lease.budgets.find((b) => b.purpose === "credits")!;
      actual = { ok: { ceiling: credits.ceiling, spent: credits.spent } };
    }

    let id: string | null = null;
    if ("error" in expected) {
      expect(actual, at).toEqual(expected);
    } else {
      expect(actual, at).toHaveProperty("ok");
      const ok = (actual as { ok: unknown }).ok;
      if (expected.ok === "grant") {
        id = (ok as cmd.CommandResults["authorize"]).id;
        expect(id, at).toMatch(/^[0-9a-f]{32}$/);
      } else if (expected.ok === "lease") {
        id = ok as string;
        expect(id, at).toMatch(/^lease-[0-9a-f]{32}$/);
      } else if ((expected.ok as { kind?: string } | null)?.kind === "established") {
        // The engine's fake Platform stamps its own times; the rule is what must match: the
        // decimal ms `$createdAt` of the reverse request the accept wrote, not of theirs.
        expect((expected.ok as { request: string }).request, at).toBe(VECTORS.request.acceptCreatedAt);
        const { kind, request } = ok as cmd.CommandResults["dashpay_accept_request"];
        expect(kind, at).toBe("established");
        expect(request, at).toMatch(/^\d+$/);
        expect(request, at).toBe(String(fixture.world.dashpay.requestTimes[from!].outgoing));
        expect(request, at).not.toBe(String(incomingBefore));
      } else {
        expect(ok, at).toEqual(expected.ok);
      }
    }
    ids.push(id);
  }
}

describe("dashpay_accept_request against the engine's vector", () => {
  it("names its regeneration command and covers the grant rules", () => {
    expect(VECTORS.comment).toContain("DW_UPDATE_VECTORS=1 cargo test -p dw-engine accept_grant_vectors");
    const codes = new Set(VECTORS.cases.flatMap((c) => c.results.flatMap((r) => ("error" in r ? [r.error.code] : []))));
    for (const code of ["platform.grant_invalid", "platform.grant_exceeded", "platform.needs_grant", "platform.lease_expired"]) expect(codes).toContain(code);
    expect(VECTORS.request.incomingCreatedAt).not.toBe(VECTORS.request.acceptCreatedAt);
  });

  for (const c of VECTORS.cases) it(c.name, () => replay(c));
});

describe("fixture grants and leases", () => {
  it("issues single-use grants under the vault passphrase, counting a wrong one as unlock does", async () => {
    installFixture({ scenario: "dashpay-registered" });
    const purpose: GrantPurposeSpec = { kind: "platformOp", maxDuffs: 0, maxCredits: VECTORS.quote };
    await expect(cmd.authorize(purpose, W, { kind: "passphrase", passphrase: "nope" })).rejects.toMatchObject({
      code: "vault.wrong_passphrase",
      data: { kind: "wrongPassphrase", failedAttempts: 1 },
    });
    await expect(cmd.authorize(purpose, null, { kind: "passphrase", passphrase: "demo" })).rejects.toMatchObject({ code: "invalid_argument" });
    const [a, b] = await cmd.authorizeSet([purpose, { kind: "spend", maxDuffs: 5 }], W, { kind: "passphrase", passphrase: "demo" });
    expect(a).toMatchObject({ walletId: W, singleUse: true, purpose });
    expect(a.id).not.toBe(b.id);
    await cmd.revokeGrant(a.id);
    await expect(cmd.beginFlow(W, "accept", [a.id])).rejects.toMatchObject({ code: "platform.grant_invalid" });
  });

  it("begins a flow as a lease the leases list shows, and signals it", async () => {
    installFixture({ scenario: "dashpay-registered" });
    const seen: AppEvent[] = [];
    await listen<AppEvent>(cmd.ENGINE_EVENT, (e) => seen.push(e.payload));
    const g = await cmd.authorize({ kind: "platformOp", maxDuffs: 0, maxCredits: VECTORS.quote }, W, { kind: "passphrase", passphrase: "demo" });
    const id = await cmd.beginFlow(W, "accept", [g.id]);
    const lease = (await cmd.leases()).find((l) => l.id === id.slice(6, 14));
    expect(lease).toMatchObject({ walletId: W, flow: "accept", state: { kind: "active" }, budgets: [{ purpose: "credits", ceiling: VECTORS.quote, spent: "0" }] });
    await cmd.endFlow(id);
    await cmd.endFlow(id);
    expect((await cmd.leases()).find((l) => l.id === id.slice(6, 14))?.state).toEqual({ kind: "ended" });
    // One as it begins, two as it ends (the engine's owner drop, then its table's `end`).
    expect(seen.filter((e) => e.kind === "leaseChanged")).toHaveLength(3);
  });
});
