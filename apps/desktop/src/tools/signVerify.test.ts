// Sign/Verify, by parity ID (the Swift `SignVerifyViewModelTests`, ported).

import { describe, expect, it, vi } from "vitest";
import type { AddressClass } from "../api/generated/AddressClass";
import type { VerifyOutcomeName } from "../api/generated/VerifyOutcomeName";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { GrantModel } from "./grant";
import { signErrorText, SignVerifyModel, SignVerifyText } from "./signVerify";
import { fakeVault } from "./testApi";

const ALICE = "yAliceAddress0000000000000000000001";
const SCRIPT = "8ScriptAddress00000000000000000003";

const classify = async (_: string, text: string): Promise<AddressClass> =>
  text.startsWith("8") ? { kind: "core", scriptHash: true } : text.startsWith("y") ? { kind: "core", scriptHash: false } : { kind: "invalid", problem: { kind: "notBech32mOrBase58" } };

function signer(lock = {}) {
  const vault = fakeVault(lock);
  const signs: [string, string, string][] = [];
  const verifyOutcome: { current: VerifyOutcomeName } = { current: "verified" };
  const api = {
    classifyAddress: vi.fn(classify),
    signMessage: vi.fn(async (_: string, address: string, message: string, grantId: string) => {
      signs.push([address, message, grantId]);
      return "H+signature==";
    }),
    verifyMessage: vi.fn(async () => ({ outcome: verifyOutcome.current, text: `text of ${verifyOutcome.current}` })),
  };
  const grants = new GrantModel(vault.api);
  return { m: new SignVerifyModel(api, grants, WALLET_A, "testnet"), api, vault, grants, signs, verifyOutcome };
}

describe("Sign/Verify (QT-099, QT-100)", () => {
  it("QT099_signWithUnencryptedWallet", async () => {
    const { m, vault, signs } = signer({ encrypted: false });
    await m.sign(ALICE, "hello");
    expect(m.signature).toBe("H+signature==");
    expect(m.signResult).toEqual({ ok: true, text: SignVerifyText.signed });
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "signMessage" }, credential: { kind: "unencrypted" } });
    expect(signs).toEqual([[ALICE, "hello", "grant-1"]]);
  });

  it("QT099_encryptedWalletNeedsThePassphrase", async () => {
    const { m, grants, vault, signs } = signer({ encrypted: true, state: "locked" });
    const signing = m.sign(ALICE, "hello");
    await vi.waitFor(() => expect(grants.pending).not.toBeNull());
    expect(signs).toEqual([]);
    await grants.submit("pw");
    await signing;
    expect(vault.calls.authorize[0].credential).toEqual({ kind: "passphrase", passphrase: "pw" });
    expect(m.signResult?.ok).toBe(true);
    const again = m.sign(ALICE, "hello");
    await vi.waitFor(() => expect(grants.pending).not.toBeNull());
    grants.cancel();
    await again;
    expect(m.signResult).toEqual({ ok: false, text: SignVerifyText.unlockCancelled });
  });

  it("QT099_addressChecksUseDashQtTexts", async () => {
    const { m, vault } = signer();
    await m.sign("nonsense", "x");
    expect(m.signResult?.text).toBe(SignVerifyText.invalidAddress);
    await m.sign(SCRIPT, "x");
    expect(m.signResult?.text).toBe(SignVerifyText.addressNoKey);
    expect(vault.calls.authorize).toEqual([]);
  });

  it("QT099_engineErrorsMapToDashQtTexts", () => {
    expect(signErrorText(appError("address_not_mine"))).toBe(SignVerifyText.privateKeyUnavailable);
    expect(signErrorText(appError("wallet_not_found"))).toBe(SignVerifyText.signingFailed);
  });

  it("L2_failedSignRevokesTheGrant", async () => {
    const { m, api, vault } = signer();
    api.signMessage.mockRejectedValueOnce(appError("address_not_mine"));
    await m.sign(ALICE, "hello");
    expect(m.signResult).toEqual({ ok: false, text: SignVerifyText.privateKeyUnavailable });
    expect(vault.calls.revoked).toEqual(["grant-1"]);
  });

  it("L2_successfulSignRevokesNothing", async () => {
    const { m, vault } = signer();
    await m.sign(ALICE, "hello");
    expect(m.signResult?.ok).toBe(true);
    expect(vault.calls.revoked).toEqual([]);
  });

  it("QT100_verify", async () => {
    const { m, verifyOutcome } = signer();
    await m.verify(ALICE, "sig", "hello");
    expect(m.verified).toBe(true);
    verifyOutcome.current = "malformedSignature";
    await m.verify(ALICE, "sig", "hello");
    expect(m.verified).toBe(false);
    expect(m.verifyResult?.text).toBe("text of malformedSignature");
    m.clearVerify();
    expect(m.verifyResult).toBeNull();
  });

  it("QT099_aSecondSignWhileOneRunsIsIgnored", async () => {
    const { m, signs, api } = signer({ encrypted: false });
    const first = m.sign(ALICE, "hello");
    await m.sign(ALICE, "hello");
    await first;
    expect(api.classifyAddress).toHaveBeenCalledTimes(1);
    expect(signs).toHaveLength(1);
    expect(m.signing).toBe(false);
  });
});
