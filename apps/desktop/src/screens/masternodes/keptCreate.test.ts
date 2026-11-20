import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { QueryClient } from "@tanstack/react-query";
import type { MyProposal } from "../../api/generated/MyProposal";
import { closeWalletGate, resetWalletGate } from "../../api/lockGate";
import { GovText } from "../../governance/model";
import { checkKept, createHeldBack, dismissKept, keptCreate, readKeptAgain, resetKeptCreate, sawMine, settleCreate, startCreate } from "./keptCreate";

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
const OLD = proposal("aa", "older");

beforeEach(() => resetWalletGate(false));

afterEach(() => {
  resetKeptCreate();
  resetWalletGate(false);
});

const unsure = () => {
  const attempt = startCreate(W, "fresh", [OLD]);
  settleCreate(W, attempt, { kind: "unsure" });
  return attempt;
};

describe("a proposal create kept across a lock", () => {
  it("holds Create back while paying, and ends as made once Mine shows the attempt's proposal, the call never settling", () => {
    const attempt = startCreate(W, "fresh", [OLD]);
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
  });

  it("does not count a proposal Mine held when Pay was pressed", () => {
    const attempt = startCreate(W, "older", [OLD]);
    sawMine(W, [OLD]);
    expect(keptCreate(W)).toEqual({ kind: "paying", attempt });
  });

  it("ignores a late answer for an attempt another replaced", () => {
    const first = startCreate(W, "fresh", []);
    const second = startCreate(W, "other", []);
    settleCreate(W, first, { kind: "unsure" });
    expect(keptCreate(W)).toEqual({ kind: "paying", attempt: second });
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
    unsure();
    await checkKept(W, async () => [OLD]);
    expect(keptCreate(W)).toMatchObject({ kind: "unsure", check: { kind: "absent" } });
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
