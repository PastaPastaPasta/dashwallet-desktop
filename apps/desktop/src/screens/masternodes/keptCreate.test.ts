import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { QueryClient } from "@tanstack/react-query";
import type { MyProposal } from "../../api/generated/MyProposal";
import { closeWalletGate, resetWalletGate } from "../../api/lockGate";
import { GovText } from "../../governance/model";
import {
  checkKept,
  createHeldBack,
  dismissKept,
  keepCreateDone,
  keptCreate,
  type ProposalPrint,
  readKeptAgain,
  resetKeptCreate,
  sawMine,
  settleCreate,
  startCreate,
} from "./keptCreate";

const W = "w1";
const proposal = (hash: string, name: string, state: MyProposal["state"] = { kind: "feePaid", confirmations: 0 }): MyProposal => ({
  hash,
  name,
  url: "https://example.org",
  paymentAddress: "yAddress",
  paymentAmount: 1,
  paymentCount: 1,
  startEpoch: 0,
  endEpoch: 1,
  feeTxid: hash.split("").reverse().join(""),
  createdAt: 0,
  submittedAt: null,
  state,
  lastError: null,
});
const draft = (name: string, more: Partial<ProposalPrint> = {}): ProposalPrint => ({ name, url: "https://example.org", paymentAddress: "yAddress", paymentAmount: 1, paymentCount: 1, ...more });
const OLD = proposal("aa", "older");

beforeEach(() => resetWalletGate(false));

afterEach(() => {
  resetKeptCreate();
  resetWalletGate(false);
});

const unsure = (name = "fresh") => {
  const attempt = startCreate(W, draft(name), [OLD]);
  settleCreate(W, attempt, { kind: "unsure" });
  return attempt;
};

/** Proposal A's fee became unsure and Mine, read since, did not show it. */
const absentA = async () => {
  const a = unsure();
  await checkKept(W, async () => [OLD]);
  expect(keptCreate(W)).toMatchObject({ kind: "unsure", check: { kind: "absent" } });
  return a;
};

describe("a proposal create kept across a lock", () => {
  it("holds Create back while paying, and ends as made once Mine shows the attempt's proposal, the call never settling", () => {
    const attempt = startCreate(W, draft("fresh"), [OLD]);
    expect(createHeldBack(keptCreate(W))).toBe(GovText.keptPaying);
    sawMine(W, [OLD]);
    expect(keptCreate(W)?.kind).toBe("paying");
    sawMine(W, [proposal("bb", "fresh", { kind: "feeFailed" }), OLD]);
    expect(keptCreate(W)?.kind).toBe("paying");
    const made = proposal("cc", "fresh");
    sawMine(W, [made, OLD]);
    expect(keptCreate(W)).toEqual({ kind: "created", p: made });
    expect(createHeldBack(keptCreate(W))).toBeNull();
    settleCreate(W, attempt, { kind: "unsure" });
    expect(keptCreate(W)).toEqual({ kind: "created", p: made });
    keepCreateDone(W);
    expect(keptCreate(W)).toBeNull();
  });

  it("does not count a proposal Mine held when Pay was pressed, nor one of another draft", () => {
    const attempt = startCreate(W, draft("older"), [OLD]);
    sawMine(W, [OLD]);
    expect(keptCreate(W)).toEqual({ kind: "paying", attempt });
    sawMine(W, [{ ...proposal("cc", "older"), url: "https://example.org/other" }, { ...proposal("dd", "older"), paymentAmount: 9 }, OLD]);
    expect(keptCreate(W)).toEqual({ kind: "paying", attempt });
  });

  it("forgets an attempt that paid nothing, and a sheet that showed its proposal leaves nothing to show", () => {
    const first = startCreate(W, draft("fresh"), []);
    settleCreate(W, first, null);
    expect(keptCreate(W)).toBeNull();
    const second = startCreate(W, draft("fresh"), []);
    settleCreate(W, second, { kind: "created", p: proposal("cc", "fresh"), shown: true });
    expect(keptCreate(W)).toBeNull();
  });

  it("unsure, holds Create back until Mine read since the unlock shows the proposal, and keeps it back while it is there", async () => {
    unsure();
    expect(createHeldBack(keptCreate(W))).toBe(GovText.createChecking);
    dismissKept(W);
    expect(keptCreate(W)?.kind).toBe("unsure");
    const made = proposal("cc", "fresh");
    await checkKept(W, async () => [made, OLD]);
    expect(keptCreate(W)).toMatchObject({ kind: "unsure", check: { kind: "found", p: made } });
    expect(createHeldBack(keptCreate(W))).toBe(GovText.createFound("fresh"));
    dismissKept(W);
    expect(keptCreate(W)).toBeNull();
  });

  it("unsure and not in Mine, lets Create go and keeps the notice until it is dismissed", async () => {
    await absentA();
    expect(createHeldBack(keptCreate(W))).toBeNull();
    const made = proposal("cc", "fresh");
    sawMine(W, [made, OLD]);
    expect(keptCreate(W)).toMatchObject({ check: { kind: "found", p: made } });
  });

  it("unsure and Mine unreadable, holds Create back, cannot be dismissed, and reads again on request", async () => {
    unsure();
    await checkKept(W, () => Promise.reject(new Error("no peer")));
    expect(keptCreate(W)).toMatchObject({ check: { kind: "failed" } });
    expect(createHeldBack(keptCreate(W))).toBe(GovText.createReadFailed);
    dismissKept(W);
    expect(keptCreate(W)?.kind).toBe("unsure");
    readKeptAgain(W);
    expect(keptCreate(W)).toMatchObject({ check: { kind: "reading" } });
    await checkKept(W, async () => [OLD]);
    expect(keptCreate(W)).toMatchObject({ check: { kind: "absent" } });
  });

  it("reads nothing while locked, and reads again when the vault locked during the read", async () => {
    unsure();
    const client = new QueryClient();
    closeWalletGate(client);
    let reads = 0;
    await checkKept(W, async () => (reads++, [OLD]));
    expect(reads).toBe(0);
    resetWalletGate(false);
    await checkKept(W, async () => {
      reads++;
      closeWalletGate(client);
      return [proposal("cc", "fresh")];
    });
    expect(reads).toBe(1);
    expect(keptCreate(W)).toMatchObject({ check: { kind: "reading" } });
    expect(createHeldBack(keptCreate(W))).toBe(GovText.createChecking);
  });

  it("starts one read at a time", async () => {
    unsure();
    let reads = 0;
    let answer = (_: MyProposal[]) => {};
    const read = () => (reads++, new Promise<MyProposal[]>((r) => (answer = r)));
    const first = checkKept(W, read);
    void checkKept(W, read);
    expect(reads).toBe(1);
    answer([OLD]);
    await first;
    expect(keptCreate(W)).toMatchObject({ check: { kind: "absent" } });
  });
});

describe("two attempts at the same draft", () => {
  it("does not credit a newer attempt with a proposal the earlier unsure one may have made: both fees may have gone out", async () => {
    await absentA();
    dismissKept(W);
    expect(keptCreate(W)).toBeNull();
    startCreate(W, draft("fresh"), [OLD]);
    const a = proposal("a1", "fresh");
    sawMine(W, [a, OLD]);
    expect(keptCreate(W)).toMatchObject({ kind: "unsure", check: { kind: "twin" } });
    expect(createHeldBack(keptCreate(W))).toBe(GovText.createTwin("fresh"));
    dismissKept(W);
    // B still pays: Create stays back.
    expect(keptCreate(W)?.kind).toBe("paying");
  });

  it("B's own answer after A appears names B's proposal, and A's is then A's", async () => {
    await absentA();
    const b = startCreate(W, draft("fresh"), [OLD]);
    const a = proposal("a1", "fresh");
    sawMine(W, [a, OLD]);
    expect(keptCreate(W)).toMatchObject({ check: { kind: "twin" } });
    const own = proposal("b1", "fresh");
    // B's sheet went with the lock: its proposal is shown on the page.
    settleCreate(W, b, { kind: "created", p: own, shown: false });
    sawMine(W, [own, a, OLD]);
    expect(keptCreate(W)).toEqual({ kind: "created", p: own });
    keepCreateDone(W);
    expect(keptCreate(W)).toMatchObject({ kind: "unsure", check: { kind: "found", p: a } });
    expect(createHeldBack(keptCreate(W))).toBe(GovText.createFound("fresh"));
    dismissKept(W);
    expect(keptCreate(W)).toBeNull();
  });

  it("A and B both appearing credit neither while B has not answered, and B's answer settles both", async () => {
    await absentA();
    const b = startCreate(W, draft("fresh"), [OLD]);
    const a = proposal("a1", "fresh");
    const own = proposal("b1", "fresh");
    sawMine(W, [own, a, OLD]);
    expect(keptCreate(W)).toMatchObject({ check: { kind: "twin" } });
    settleCreate(W, b, { kind: "unsure" });
    await checkKept(W, async () => [own, a, OLD]);
    expect(keptCreate(W)).toMatchObject({ check: { kind: "twin" } });
    expect(createHeldBack(keptCreate(W))).toBe(GovText.createTwin("fresh"));
  });

  it("A and B both appearing, with B's answer, show each as made", async () => {
    await absentA();
    const b = startCreate(W, draft("fresh"), [OLD]);
    const a = proposal("a1", "fresh");
    const own = proposal("b1", "fresh");
    settleCreate(W, b, { kind: "created", p: own, shown: true });
    sawMine(W, [own, a, OLD]);
    expect(keptCreate(W)).toMatchObject({ kind: "unsure", check: { kind: "found", p: a } });
  });

  it("B's own answer, Mine unchanged since it was read, ends the doubt at once", async () => {
    await absentA();
    const b = startCreate(W, draft("fresh"), [OLD]);
    const own = proposal("b1", "fresh");
    sawMine(W, [own, OLD]);
    expect(keptCreate(W)).toMatchObject({ check: { kind: "twin" } });
    settleCreate(W, b, { kind: "created", p: own, shown: true });
    expect(keptCreate(W)).toMatchObject({ kind: "unsure", check: { kind: "absent" } });
  });

  it("A never appearing leaves A unsure and not found, B's proposal being B's", async () => {
    await absentA();
    const b = startCreate(W, draft("fresh"), [OLD]);
    const own = proposal("b1", "fresh");
    settleCreate(W, b, { kind: "created", p: own, shown: true });
    sawMine(W, [own, OLD]);
    expect(keptCreate(W)).toMatchObject({ kind: "unsure", check: { kind: "absent" } });
    expect(createHeldBack(keptCreate(W))).toBeNull();
  });

  it("an earlier attempt still being read holds B's crediting until its read answers", async () => {
    unsure();
    const b = startCreate(W, draft("fresh"), [OLD]);
    expect(createHeldBack(keptCreate(W))).toBe(GovText.createChecking);
    const a = proposal("a1", "fresh");
    sawMine(W, [a, OLD]);
    expect(keptCreate(W)).toMatchObject({ check: { kind: "reading" } });
    await checkKept(W, async () => [a, OLD]);
    expect(keptCreate(W)).toMatchObject({ check: { kind: "twin" } });
    settleCreate(W, b, { kind: "created", p: proposal("b1", "fresh"), shown: true });
    sawMine(W, [proposal("b1", "fresh"), a, OLD]);
    expect(keptCreate(W)).toMatchObject({ check: { kind: "found", p: a } });
  });
});
