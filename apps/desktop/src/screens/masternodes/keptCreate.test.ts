import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { QueryClient } from "@tanstack/react-query";
import type { MyProposal } from "../../api/generated/MyProposal";
import { closeWalletGate, resetWalletGate } from "../../api/lockGate";
import { GovText } from "../../governance/model";
import {
  acknowledge,
  attemptStatus,
  beginCreate,
  checkKept,
  forgetKeptCreate,
  goOnWithAnother,
  keepsAnything,
  keptCreate,
  ledgerSnapshot,
  matches,
  priorRisk,
  type ProposalPrint,
  readKeptAgain,
  readMine,
  resetKeptCreate,
  settleCreate,
} from "./keptCreate";

const W = "w1";
const CYCLE = 100;
const print = (name = "fresh", more: Partial<ProposalPrint> = {}): ProposalPrint => ({
  name,
  url: "https://example.org",
  paymentAddress: "yAddress",
  paymentAmount: 100_000_000,
  paymentCount: 1,
  startEpoch: 1000,
  endEpoch: 1000 + CYCLE,
  ...more,
});
const row = (hash: string, p: ProposalPrint = print(), state: MyProposal["state"] = { kind: "feePaid", confirmations: 0 }): MyProposal => ({
  hash,
  name: p.name,
  url: p.url,
  paymentAddress: p.paymentAddress,
  paymentAmount: p.paymentAmount,
  paymentCount: p.paymentCount,
  startEpoch: p.startEpoch,
  endEpoch: p.endEpoch,
  feeTxid: `fee-${hash}`,
  createdAt: 0,
  submittedAt: null,
  state,
  lastError: null,
});
/** A proposal of this draft from before the session. */
const OLD = row("old");

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((a, b) => ((resolve = a), (reject = b)));
  return { promise, resolve, reject };
}

const lockAndUnlock = () => {
  closeWalletGate(new QueryClient());
  resetWalletGate(false);
};

async function go(p: ProposalPrint = print(), rows: MyProposal[] = [OLD]): Promise<number> {
  const r = await beginCreate(W, p, async () => rows);
  if (r.kind !== "go") throw new Error(`not paid: ${JSON.stringify(r)}`);
  return r.attempt;
}

/** An attempt whose call failed after its fee may have gone out, and Mine, read since, without it. */
async function absent(p: ProposalPrint = print()): Promise<number> {
  const a = await go(p);
  settleCreate(W, a, { kind: "unsure" });
  await checkKept(W, async () => [OLD]);
  expect(attemptStatus(W, a)?.status).toBe("absent");
  return a;
}

beforeEach(() => resetWalletGate(false));

afterEach(() => {
  resetKeptCreate();
  resetWalletGate(false);
});

describe("1: Pay goes out only after a current Mine read, reconciled, held and recorded at once", () => {
  it("pays nothing when Mine cannot be read, or the vault locks during the read", async () => {
    expect(await beginCreate(W, print(), () => Promise.reject(new Error("no peer")))).toEqual({ kind: "unread" });
    expect(
      await beginCreate(W, print(), async () => {
        closeWalletGate(new QueryClient());
        return [];
      }),
    ).toEqual({ kind: "unread" });
    resetWalletGate(false);
    expect(keptCreate(W)).toBeNull();
  });

  it("an earlier attempt's proposal that appears while the next sheet is open stops the next Pay", async () => {
    const a = await absent();
    acknowledge(W, [a]);
    expect(keptCreate(W)).toBeNull();
    // The page's Mine is not read again before B's Pay; B's own read is, and it shows A's proposal.
    const r = await beginCreate(W, print(), async () => [row("a1"), OLD]);
    expect(r).toEqual({ kind: "held", reason: GovText.sameDraftHeld("fresh") });
    expect(attemptStatus(W, a)).toEqual({ status: "found", hash: "a1" });
    expect(keptCreate(W)?.held).toBe(GovText.createFound("fresh"));
  });

  it("an earlier attempt not known yet holds every draft, and its own draft whatever else", async () => {
    const a = await go();
    expect(await beginCreate(W, print("other"), async () => [OLD])).toEqual({ kind: "held", reason: GovText.keptPaying });
    settleCreate(W, a, { kind: "unsure" });
    expect(await beginCreate(W, print("other"), () => Promise.reject(new Error("no peer")))).toEqual({ kind: "unread" });
    expect(attemptStatus(W, a)?.status).toBe("reading");
  });

  it("a read that a newer one overtook is read again before Pay", async () => {
    const first = deferred<MyProposal[]>();
    let reads = 0;
    const pending = beginCreate(W, print(), () => (++reads === 1 ? first.promise : Promise.resolve([OLD])));
    await readMine(W, async () => [OLD]);
    first.resolve([]);
    expect((await pending).kind).toBe("go");
    expect(reads).toBe(2);
  });

  it("a proposal of the draft Mine held at Pay is never the attempt's, nor one Mine could not show then", async () => {
    const a = await go(print(), [OLD]);
    await readMine(W, async () => [OLD]);
    expect(attemptStatus(W, a)?.status).toBe("paying");
    await readMine(W, async () => [row("a1"), OLD]);
    expect(attemptStatus(W, a)).toEqual({ status: "created", hash: "a1" });
  });
});

describe("2: Mine observations apply in the order asked; an uncertain mark goes only by an attempt's own answer", () => {
  async function twoUnsure() {
    const a = await absent();
    acknowledge(W, [a]);
    const b = await go();
    settleCreate(W, b, { kind: "unsure" });
    return { a, b };
  }

  it("an older read landing after a newer one is dropped, and cannot take the warning away", async () => {
    const { a, b } = await twoUnsure();
    const older = deferred<MyProposal[]>();
    const page = readMine(W, () => older.promise);
    await checkKept(W, async () => [row("x1"), OLD]);
    expect(keptCreate(W)).toMatchObject({ status: "uncertain", ids: [a, b] });
    older.resolve([OLD]);
    expect((await page).applied).toBe(false);
    expect(keptCreate(W)).toMatchObject({ status: "uncertain", held: GovText.createTwin("fresh") });
  });

  it("a later list without the proposal does not clear the mark; the other attempt's answer does", async () => {
    const a = await go();
    const b = await go(print(), [OLD]).catch(() => null);
    expect(b).toBeNull(); // A pays: the draft is held
    goOnWithAnother(W, [a]);
    expect(await beginCreate(W, print(), async () => [OLD])).toEqual({ kind: "held", reason: GovText.sameDraftHeld("fresh") });
    settleCreate(W, a, { kind: "unsure" });
    await checkKept(W, async () => [OLD]);
    acknowledge(W, [a]);
    const c = await go();
    await readMine(W, async () => [row("x1"), OLD]);
    expect(attemptStatus(W, a)?.status).toBe("uncertain");
    expect(attemptStatus(W, c)?.status).toBe("uncertain");
    await readMine(W, async () => [OLD]);
    expect(attemptStatus(W, c)?.status).toBe("uncertain");
    // C's own answer names another proposal: x1 is A's.
    settleCreate(W, c, { kind: "created", hash: "c1", shown: true });
    await readMine(W, async () => [row("c1"), row("x1"), OLD]);
    expect(attemptStatus(W, a)).toEqual({ status: "found", hash: "x1" });
    expect(attemptStatus(W, c)).toEqual({ status: "created", hash: "c1" });
  });

  it("three attempts and one proposal: one notice for all three, and its copy does not say two", async () => {
    const ids = [await absent()];
    for (let i = 0; i < 2; i++) {
      acknowledge(W, ids);
      const n = await go();
      settleCreate(W, n, { kind: "unsure" });
      await checkKept(W, async () => [OLD]);
      ids.push(n);
    }
    await readMine(W, async () => [row("x1"), OLD]);
    expect(keptCreate(W)).toMatchObject({ status: "uncertain", ids, dismissable: true });
    expect(GovText.keptTwin("fresh")).toContain("more than one attempt may have paid a fee");
    acknowledge(W, ids);
    expect(keptCreate(W)).toBeNull();
    expect(priorRisk(W, "fresh")).toBe(GovText.priorFee("fresh"));
  });

  it("a lock makes a finished check stale: Create waits for a read since the unlock", async () => {
    const a = await absent();
    lockAndUnlock();
    expect(attemptStatus(W, a)?.status).toBe("reading");
    expect(keptCreate(W)?.held).toBe(GovText.createChecking);
    const during = deferred<MyProposal[]>();
    const check = checkKept(W, () => during.promise);
    lockAndUnlock();
    during.resolve([OLD]);
    await check;
    expect(attemptStatus(W, a)?.status).toBe("reading");
    await checkKept(W, async () => [OLD]);
    expect(attemptStatus(W, a)?.status).toBe("absent");
  });

  it("reads nothing while locked", async () => {
    await go().then((a) => settleCreate(W, a, { kind: "unsure" }));
    closeWalletGate(new QueryClient());
    let reads = 0;
    await checkKept(W, async () => (reads++, [OLD]));
    expect(reads).toBe(0);
  });
});

describe("3: each attempt's outcome is its own, and a button acknowledges what it shows", () => {
  it("B's sheet shows B's proposal, and acknowledging A's leaves B's", async () => {
    const a = await absent();
    acknowledge(W, [a]);
    const other = print("fresh", { paymentAddress: "yOther" });
    const b = await go(other, [OLD]);
    settleCreate(W, b, { kind: "unsure" });
    await checkKept(W, async () => [row("b1", other), row("a1"), OLD]);
    expect(attemptStatus(W, a)).toEqual({ status: "found", hash: "a1" });
    expect(attemptStatus(W, b)).toEqual({ status: "found", hash: "b1" });
    acknowledge(W, [a]);
    expect(keptCreate(W)).toMatchObject({ status: "found", ids: [b], hash: "b1", held: GovText.createFound("fresh") });
    acknowledge(W, [b]);
    expect(keptCreate(W)).toBeNull();
  });

  it("an outcome not known cannot be acknowledged", async () => {
    const a = await go();
    acknowledge(W, [a]);
    expect(keptCreate(W)).toMatchObject({ status: "paying", dismissable: false, waiting: true, held: GovText.keptPaying });
    settleCreate(W, a, { kind: "unsure" });
    await checkKept(W, () => Promise.reject(new Error("no peer")));
    acknowledge(W, [a]);
    expect(keptCreate(W)).toMatchObject({ status: "failed", held: GovText.createReadFailed });
    readKeptAgain(W);
    expect(keptCreate(W)?.status).toBe("reading");
  });

  it("forgets an attempt that paid nothing, and one whose sheet showed its proposal", async () => {
    settleCreate(W, await go(), null);
    expect(keptCreate(W)).toBeNull();
    settleCreate(W, await go(), { kind: "created", hash: "c1", shown: true });
    expect(keptCreate(W)).toBeNull();
  });

  it("a proposal Mine showed while the call was out, the call then failing, is told as found", async () => {
    const a = await go();
    await readMine(W, async () => [row("a1"), OLD]);
    expect(attemptStatus(W, a)?.status).toBe("created");
    settleCreate(W, a, { kind: "unsure" });
    expect(attemptStatus(W, a)).toEqual({ status: "found", hash: "a1" });
    expect(keptCreate(W)?.held).toBe(GovText.createFound("fresh"));
  });

  it("a proposal made while no sheet showed it is the page's to show", async () => {
    const a = await go();
    settleCreate(W, a, { kind: "created", hash: "c1", shown: false });
    expect(keptCreate(W)).toMatchObject({ status: "created", ids: [a], hash: "c1", held: null });
  });
});

describe("4: the ledger holds what outstanding attempts need, and a forgotten wallet stays forgotten", () => {
  it("keeps nothing of Mine's rows", async () => {
    const secret = row("zz", print("someone-else", { paymentAddress: "ySecret" }));
    await readMine(W, async () => [secret]);
    const a = await go(print(), [secret, OLD]);
    await readMine(W, async () => [secret, OLD]);
    settleCreate(W, a, { kind: "unsure" });
    expect(ledgerSnapshot(W)).toContain("fresh");
    expect(ledgerSnapshot(W)).not.toMatch(/someone-else|ySecret|fee-/);
    expect(priorRisk(W, "someone-else")).toBeNull();
  });

  it("a removed wallet forgets its attempts, and a read still out brings nothing back", async () => {
    const a = await go();
    settleCreate(W, a, { kind: "unsure" });
    const late = deferred<MyProposal[]>();
    const check = checkKept(W, () => late.promise);
    const page = readMine(W, async () => [row("a1"), OLD]);
    forgetKeptCreate(W);
    await page;
    late.reject(new Error("gone"));
    await check;
    expect(keptCreate(W)).toBeNull();
    expect(attemptStatus(W, a)).toBeNull();
    expect(keepsAnything(W)).toBe(false);
  });

  it("the vault going forgets every wallet", async () => {
    await go();
    await beginCreate("w2", print(), async () => []);
    const late = deferred<MyProposal[]>();
    const page = readMine("w2", () => late.promise);
    forgetKeptCreate();
    late.resolve([row("a1")]);
    expect((await page).applied).toBe(false);
    expect(keepsAnything(W) || keepsAnything("w2")).toBe(false);
  });
});

describe("5: a draft matches a proposal exactly, and what still collides is uncertain", () => {
  it("tells apart one duff, a later first payment, and anything else entered", () => {
    const p = print();
    expect(matches(p, row("a", p))).toBe(true);
    expect(matches(p, row("a", { ...p, startEpoch: p.startEpoch + CYCLE / 5 }))).toBe(true); // the tip moved
    expect(matches(p, row("a", { ...p, paymentAmount: p.paymentAmount + 1 }))).toBe(false);
    expect(matches(p, row("a", { ...p, startEpoch: p.startEpoch + CYCLE, endEpoch: p.endEpoch + CYCLE }))).toBe(false);
    expect(matches(p, row("a", { ...p, paymentCount: 2 }))).toBe(false);
    expect(matches(p, row("a", { ...p, url: "https://example.org/x" }))).toBe(false);
    expect(matches(p, row("a", { ...p, paymentAddress: "yOther" }))).toBe(false);
  });

  it("an attempt two new proposals could be is uncertain, not made", async () => {
    const a = await go();
    await readMine(W, async () => [row("a1"), row("a2"), OLD]);
    expect(attemptStatus(W, a)?.status).toBe("uncertain");
    settleCreate(W, a, { kind: "created", hash: "a2", shown: false });
    expect(attemptStatus(W, a)).toEqual({ status: "created", hash: "a2" });
  });

  it("does not count a proposal whose fee failed", async () => {
    const a = await go();
    await readMine(W, async () => [row("a1", print(), { kind: "feeFailed" }), OLD]);
    expect(attemptStatus(W, a)?.status).toBe("paying");
  });
});

describe("6: an attempt that never settles leaves a way on, its draft still held", () => {
  it("goes on with a different proposal while the call never answers", async () => {
    const a = await go();
    expect(keptCreate(W)?.held).toBe(GovText.keptPaying);
    goOnWithAnother(W, [a]);
    expect(keptCreate(W)).toMatchObject({ status: "paying", ids: [a], waiting: false, held: null });
    expect(await beginCreate(W, print(), async () => [OLD])).toEqual({ kind: "held", reason: GovText.sameDraftHeld("fresh") });
    const b = await go(print("other"));
    expect(attemptStatus(W, b)?.status).toBe("paying");
    expect(attemptStatus(W, a)?.status).toBe("paying");
  });

  it("an attempt the user went on from does not hide a notice that holds Create", async () => {
    const a = await go(print("stuck"));
    goOnWithAnother(W, [a]);
    const b = await go();
    await readMine(W, async () => [row("b1"), row("b2"), OLD]);
    expect(keptCreate(W)).toMatchObject({ status: "uncertain", ids: [b], held: GovText.createTwin("fresh") });
    settleCreate(W, b, { kind: "unsure" });
    acknowledge(W, [b]);
    expect(keptCreate(W)).toMatchObject({ status: "paying", ids: [a], waiting: false, held: null });
  });

  it("a fee that turns out to have gone out is told again, whatever the user went on with", async () => {
    const a = await go();
    settleCreate(W, a, { kind: "unsure" });
    await checkKept(W, () => Promise.reject(new Error("no peer")));
    goOnWithAnother(W, [a]);
    expect(keptCreate(W)?.held).toBeNull();
    readKeptAgain(W);
    await checkKept(W, async () => [row("a1"), OLD]);
    expect(keptCreate(W)).toMatchObject({ status: "found", held: GovText.createFound("fresh") });
  });
});
