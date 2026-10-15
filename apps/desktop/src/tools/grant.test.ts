// GrantModel's request lifecycle (U-05 D7, D15, D17, D12): one request at a time, and a grant
// that arrives for a request no longer waiting is revoked, never handed on.

import { describe, expect, it, vi } from "vitest";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { GrantModel } from "./grant";
import { fakeVault } from "./testApi";

const SPEND = { kind: "spend", maxDuffs: 1000 } as const;

const until = async (cond: () => boolean) => {
  for (let i = 0; i < 100 && !cond(); i++) await new Promise((r) => setTimeout(r, 0));
  expect(cond()).toBe(true);
};

describe("GrantModel", () => {
  it("a second request while one is asked for is refused at once", async () => {
    const { api } = fakeVault();
    const grants = new GrantModel(api);
    const first = grants.obtain(SPEND, WALLET_A);
    await expect(grants.obtain(SPEND, WALLET_A)).rejects.toThrow("already being asked for");
    await until(() => grants.pending !== null);
    grants.cancel();
    expect(await first).toBeNull();
  });

  it("a cancel while the passphrase is checked revokes the grant it brings", async () => {
    const { api, calls } = fakeVault();
    const grants = new GrantModel(api);
    const answer = grants.obtain(SPEND, WALLET_A);
    await until(() => grants.pending !== null);
    const submitted = grants.submit("pw");
    grants.cancel();
    expect(await submitted).toBe(false);
    expect(await answer).toBeNull();
    expect(calls.revoked).toEqual(["grant-1"]);

    // The next request is not answered with that grant.
    const next = grants.obtain(SPEND, WALLET_A);
    await until(() => grants.pending !== null);
    expect(grants.pending).not.toBeNull();
    grants.cancel();
    expect(await next).toBeNull();
  });

  it("a cancel before the prompt shows ends the request; a grant issued meanwhile is revoked", async () => {
    const prompt = fakeVault();
    const asking = new GrantModel(prompt.api);
    const asked = asking.obtain(SPEND, WALLET_A);
    asking.cancel();
    expect(await asked).toBeNull();
    expect(asking.pending).toBeNull();

    const unencrypted = fakeVault({ encrypted: false });
    const authorize = unencrypted.api.authorize.getMockImplementation()!;
    let release = () => {};
    unencrypted.api.authorize.mockImplementationOnce(async (...args) => {
      await new Promise<void>((r) => (release = r));
      return authorize(...args);
    });
    const silent = new GrantModel(unencrypted.api);
    const issued = silent.obtain({ kind: "signMessage" }, WALLET_A);
    await until(() => unencrypted.api.authorize.mock.calls.length === 1);
    silent.cancel();
    release();
    expect(await issued).toBeNull();
    await until(() => unencrypted.calls.revoked.length === 1);
    expect(unencrypted.calls.revoked).toEqual(["grant-1"]);
  });

  it("withGrant revokes a grant its call did not use, and passes a cancel on as null", async () => {
    const { api, calls } = fakeVault({ encrypted: false });
    const grants = new GrantModel(api);
    await expect(
      grants.withGrant(SPEND, WALLET_A, async () => {
        throw new Error("refused");
      }),
    ).rejects.toThrow("refused");
    await until(() => calls.revoked.length === 1);
    expect(await grants.withGrant(SPEND, WALLET_A, async (g) => g.id)).toEqual({ value: "grant-2" });

    const asked = fakeVault();
    const prompting = new GrantModel(asked.api);
    const run = vi.fn(async () => "ran");
    const cancelled = prompting.withGrant(SPEND, WALLET_A, run);
    await until(() => prompting.pending !== null);
    prompting.cancel();
    expect(await cancelled).toBeNull();
    expect(run).not.toHaveBeenCalled();
  });

  it("a request made after a cancel that came after its action began is not asked (since)", async () => {
    const { api } = fakeVault();
    const grants = new GrantModel(api);
    const since = grants.epoch;
    grants.cancel(); // the tab or wallet switched while the action awaited something else
    expect(await grants.obtain(SPEND, WALLET_A, since)).toBeNull();
    expect(await grants.withGrant(SPEND, WALLET_A, async () => "ran", since)).toBeNull();
    expect(grants.pending).toBeNull();
    expect(api.lockStatus).not.toHaveBeenCalled();
    // A request from an action begun after the cancel is asked as usual.
    const next = grants.obtain(SPEND, WALLET_A, grants.epoch);
    await until(() => grants.pending !== null);
    grants.cancel();
    expect(await next).toBeNull();
  });

  it("a cancel while the vault's credential is refused does not fall through to the prompt", async () => {
    const { api } = fakeVault({ encrypted: false });
    let refuse = () => {};
    api.authorize.mockImplementationOnce(async () => {
      await new Promise<void>((r) => (refuse = r));
      throw appError("vault.locked");
    });
    const grants = new GrantModel(api);
    const asked = grants.obtain({ kind: "signMessage" }, WALLET_A);
    await until(() => api.authorize.mock.calls.length === 1);
    grants.cancel();
    refuse();
    expect(await asked).toBeNull();
    expect(grants.pending).toBeNull();
  });
});
