// PSBT, by parity ID (the Swift `PSBTAndToolsTests` PSBT cases, ported).

import { describe, expect, it, vi } from "vitest";
import type { LockStatus } from "../api/generated/LockStatus";
import type { PsbtAnalysis } from "../api/generated/PsbtAnalysis";
import { bytesToBase64, textToBase64 } from "../app/files";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { GrantModel } from "./grant";
import { PsbtModel, psbtErrorText, psbtOfFile, PsbtText } from "./psbt";
import { fakeVault } from "./testApi";

const UNSIGNED = "cHNidP8BAHUCAAAAAQ==";
const SIGNED = "cHNidP8BAHUCAAAAAg==";
const TXID = "ab".repeat(32);

const analysis = (patch: Partial<PsbtAnalysis> = {}): PsbtAnalysis => ({
  outputs: [
    { address: "yPayee000000000000000000000000001", amount: 150_000_000, isMine: false },
    { address: "yChange00000000000000000000000002", amount: 1_000, isMine: true },
  ],
  fee: 226,
  total: 150_000_226,
  unsignedInputs: 2,
  status: "needsSignatures",
  signability: "canSign",
  externalSent: 150_000_000,
  ...patch,
});

/** Unencrypted by default: no passphrase prompt. */
function psbt(lock: Partial<LockStatus> = { encrypted: false }, walletId: string | null = WALLET_A) {
  const vault = fakeVault(lock);
  const byPsbt: Record<string, PsbtAnalysis> = {
    [UNSIGNED]: analysis(),
    [SIGNED]: analysis({ unsignedInputs: 0, status: "complete" }),
  };
  const saved: [string, string, string][] = [];
  const api = {
    analyzePsbt: vi.fn(async (_: string | null, p: string) => {
      const a = byPsbt[p];
      if (!a) throw appError("psbt.invalid");
      return a;
    }),
    signPsbt: vi.fn(async (_: string, p: string, grantId: string) => {
      void grantId;
      return p === UNSIGNED ? SIGNED : p;
    }),
    broadcastPsbt: vi.fn(async () => TXID),
    saveFileAs: vi.fn(async (purpose: string, name: string, data: string): Promise<string | null> => {
      saved.push([purpose, name, data]);
      return `/home/u/${name}`;
    }),
  };
  const grants = new GrantModel(vault.api);
  return { m: new PsbtModel(api, grants, walletId, "test"), api, vault, grants, byPsbt, saved };
}

describe("PSBT (QT-076…079)", () => {
  it("QT077_createUnsignedCopiesBase64AndOffersSave", async () => {
    const { m } = psbt();
    await m.loadUnsigned(UNSIGNED, true);
    expect(m.step).toBe("ready");
    expect(m.psbt).toBe(UNSIGNED);
    expect(m.message).toBe(PsbtText.unsignedCopied);
  });

  it("QT077_aFailedCopyDoesNotSayCopied", async () => {
    const { m } = psbt();
    await m.loadUnsigned(UNSIGNED, false);
    expect(m.step).toBe("ready");
    expect(m.message).toBeNull();
  });

  it("QT078_loadFromClipboardNeedsBase64", async () => {
    const { m, api } = psbt();
    await m.loadText("not base64!");
    expect(m.error).toBe(PsbtText.pasteInvalid);
    await m.loadText("   ");
    expect(m.error).toBe(PsbtText.pasteInvalid);
    expect(api.analyzePsbt).not.toHaveBeenCalled();
    await m.loadText("bm90IGEgcHNidA==");
    expect(m.error).toBe(PsbtText.loadFailed(PsbtText.invalid));
    expect(m.step).toBe("empty");
    await m.loadText(` ${UNSIGNED.slice(0, 8)}\n${UNSIGNED.slice(8)} `);
    expect(m.psbt).toBe(UNSIGNED);
    expect(m.error).toBeNull();
  });

  it("QT078_fileIsBinaryOrBase64Text", async () => {
    const binary = bytesToBase64(new Uint8Array([0x70, 0x73, 0x62, 0x74, 0xff, 0x01, 0x00]));
    expect(psbtOfFile({ name: "a.psbt", data: binary })).toBe(binary);
    expect(psbtOfFile({ name: "a.txt", data: textToBase64(`${UNSIGNED}\n`) })).toBe(UNSIGNED);
    const { m } = psbt();
    await m.loadFile({ name: "a.txt", data: textToBase64(UNSIGNED) });
    expect(m.step).toBe("ready");
  });

  it("QT078_tooLargeFileIsRefusedByTheEngineRule", () => {
    const { m } = psbt();
    m.fileFailed(appError("app.file_too_large"));
    expect(m.error).toBe(PsbtText.loadFailed(PsbtText.tooLarge));
    m.fileFailed(appError("app.file_read"));
    expect(m.error).toBe(PsbtText.loadFailed("The file could not be read."));
  });

  it("QT079_dialogLinesAndStatusFollowDashQt", async () => {
    const { m, byPsbt } = psbt();
    await m.loadText(UNSIGNED);
    expect(m.lines).toEqual([
      " * Sends 1.50000000 tDASH to yPayee000000000000000000000000001",
      " * Sends 0.00001000 tDASH to yChange00000000000000000000000002 (own address)",
      "Pays transaction fee: 0.00000226 tDASH",
      "Total Amount: 1.50000226 tDASH",
      "Transaction has 2 unsigned inputs.",
    ]);
    expect(m.status).toBe(PsbtText.needsSignatures);
    expect(m.suggestedFileName).toBe("yPayee000000000000000000000000001-1.50000000 tDASH.psbt");
    for (const [signability, but] of [
      ["noWallet", PsbtText.noWallet],
      ["watchOnly", PsbtText.cannotSign],
      ["noMatchingKeys", PsbtText.noMatchingKeys],
    ] as const) {
      byPsbt[UNSIGNED] = analysis({ signability });
      await m.loadText(UNSIGNED);
      expect(m.status).toBe(`${PsbtText.needsSignatures} ${but}`);
      expect(m.canSign).toBe(false);
    }
    // Signed but not finalized: the engine still counts the input; complete says no line (D26).
    byPsbt[UNSIGNED] = analysis({ unsignedInputs: 1, status: "complete" });
    await m.loadText(UNSIGNED);
    expect(m.lines.some((l) => l.includes("unsigned inputs"))).toBe(false);
    byPsbt[UNSIGNED] = analysis({ outputs: [{ address: null, amount: 5, isMine: false }], fee: null, total: null, status: "missingInputInfo" });
    await m.loadText(UNSIGNED);
    expect(m.lines).toEqual([" * Sends 0.00000005 tDASH to unknown address", PsbtText.feeUnknown, "Transaction has 2 unsigned inputs."]);
    expect(m.status).toBe(PsbtText.missingInputInfo);
  });

  it("QT079_signNeedsASpendGrantCoveringTheOutflow", async () => {
    // Payment authentication is on: an unlocked encrypted wallet still asks.
    const { m, vault, api, grants } = psbt({ encrypted: true, state: "unlocked" });
    await m.loadText(UNSIGNED);
    expect(m.canSign).toBe(true);
    const signing = m.sign();
    await vi.waitFor(() => expect(grants.pending).not.toBeNull());
    await grants.submit("pw");
    await signing;
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "spend", maxDuffs: 150_000_226 }, walletId: WALLET_A, credential: { kind: "passphrase" } });
    expect(api.signPsbt).toHaveBeenCalledWith(WALLET_A, UNSIGNED, "grant-1");
    expect(m.psbt).toBe(SIGNED);
    expect(m.message).toBe(PsbtText.signedComplete);
    expect(m.status).toBe(PsbtText.complete);
    expect(m.canBroadcast).toBe(true);
  });

  it("QT079_signReportsPartialAndNone", async () => {
    const { m, byPsbt, api } = psbt();
    byPsbt[SIGNED] = analysis({ unsignedInputs: 1 });
    await m.loadText(UNSIGNED);
    await m.sign();
    expect(m.message).toBe(PsbtText.signedPartially(1));
    api.signPsbt.mockResolvedValueOnce(SIGNED);
    await m.sign();
    expect(m.message).toBe(PsbtText.couldNotSign);
  });

  it("QT079_aFailedReadBackKeepsTheSignaturesAndActsOnNothingStale", async () => {
    const { m, api } = psbt();
    await m.loadText(UNSIGNED);
    api.analyzePsbt.mockRejectedValueOnce(appError("psbt.invalid"));
    await m.sign();
    expect(m.psbt).toBe(SIGNED);
    expect(m.analysis).toBeNull();
    expect(m.canSign).toBe(false);
    expect(m.canBroadcast).toBe(false);
    expect(m.error).not.toBeNull();
  });

  it("QT079_failedSignRevokesTheGrantAndLockedSaysSo", async () => {
    const { m, api, vault } = psbt();
    api.signPsbt.mockRejectedValueOnce(appError("vault.locked"));
    await m.loadText(UNSIGNED);
    await m.sign();
    expect(m.error).toBe("Cannot sign inputs while wallet is locked.");
    expect(vault.calls.revoked).toEqual(["grant-1"]);
    expect(m.step).toBe("ready");
  });

  it("QT079_cancelledPassphraseSignsNothing", async () => {
    const { m, grants, api } = psbt({ encrypted: true, state: "locked" });
    await m.loadText(UNSIGNED);
    const signing = m.sign();
    await vi.waitFor(() => expect(grants.pending).not.toBeNull());
    grants.cancel();
    await signing;
    expect(api.signPsbt).not.toHaveBeenCalled();
    expect(m.step).toBe("ready");
  });

  it("QT079_unknownFeeIsNotSigned", async () => {
    const { m, byPsbt, vault, api } = psbt();
    byPsbt[UNSIGNED] = analysis({ fee: null, total: null });
    await m.loadText(UNSIGNED);
    await m.sign();
    expect(m.error).toBe(PsbtText.noExternalAmount);
    expect(vault.calls.authorize).toEqual([]);
    expect(api.signPsbt).not.toHaveBeenCalled();
  });

  it("QT079_noWalletLoadedCannotSign", async () => {
    const { m } = psbt({}, null);
    await m.loadText(UNSIGNED);
    expect(m.canSign).toBe(false);
  });

  it("QT079_broadcastReportsTheTxidOrTheFailure", async () => {
    const ok = psbt();
    await ok.m.loadText(SIGNED);
    await ok.m.broadcast();
    expect(ok.m.step).toBe("broadcast");
    expect(ok.m.message).toBe(PsbtText.broadcastSucceeded(TXID));
    expect(ok.m.canBroadcast).toBe(false);

    const rejected = psbt();
    rejected.api.broadcastPsbt.mockRejectedValueOnce(appError("psbt.broadcast_rejected"));
    await rejected.m.loadText(SIGNED);
    await rejected.m.broadcast();
    expect(rejected.m.error).toBe(PsbtText.broadcastFailed("The network rejected the transaction."));
    expect(rejected.m.canBroadcast).toBe(true);

    const unknown = psbt();
    unknown.api.broadcastPsbt.mockRejectedValueOnce(appError("psbt.broadcast_unknown"));
    await unknown.m.loadText(SIGNED);
    await unknown.m.broadcast();
    expect(unknown.m.sent).toBe(true);
    expect(unknown.m.notice).toBe(psbtErrorText(appError("psbt.broadcast_unknown")));
    expect(unknown.m.error).toBeNull();
    expect(unknown.m.canBroadcast).toBe(false);
    // Loading the same PSBT again offers no second broadcast either.
    await unknown.m.loadText(SIGNED);
    expect(unknown.m.canBroadcast).toBe(false);
    expect(unknown.api.broadcastPsbt).toHaveBeenCalledTimes(1);

    // The screen's models share one map: a later tab (a new model) remembers the broadcast too.
    const session = new Map<string, string | true>();
    const first = psbt();
    const sentBy = new PsbtModel(first.api, first.grants, WALLET_A, "test", session);
    await sentBy.loadText(SIGNED);
    await sentBy.broadcast();
    const later = new PsbtModel(first.api, first.grants, WALLET_A, "test", session);
    await later.loadText(SIGNED);
    expect(later.sent).toBe(TXID);
    expect(later.canBroadcast).toBe(false);
  });

  it("QT079_copyAndClose", async () => {
    const { m } = psbt();
    await m.loadText(UNSIGNED);
    m.copied();
    expect(m.message).toBe(PsbtText.copied);
    m.close();
    expect([m.step, m.psbt, m.analysis, m.message]).toEqual(["empty", null, null, null]);
  });

  it("QT077_saveWritesTheBinaryPsbtThroughTheSaveDialog (D2: the OS asks before replacing)", async () => {
    const { m, api, saved } = psbt();
    await m.loadText(UNSIGNED);
    await m.save();
    expect(saved).toEqual([["psbt", "yPayee000000000000000000000000001-1.50000000 tDASH.psbt", UNSIGNED]]);
    expect(m.message).toBe(PsbtText.saved);
    m.copied();
    api.saveFileAs.mockResolvedValueOnce(null);
    await m.save();
    expect(m.message).toBe(PsbtText.copied);
    api.saveFileAs.mockRejectedValueOnce(appError("app.file_write"));
    await m.save();
    expect(m.error).toBe(PsbtText.saveFailed);
  });

  it("a failed load keeps the PSBT already open", async () => {
    const { m } = psbt();
    await m.loadText(UNSIGNED);
    await m.loadText("bm90IGEgcHNidA==");
    expect(m.step).toBe("ready");
    expect(m.psbt).toBe(UNSIGNED);
  });
});
