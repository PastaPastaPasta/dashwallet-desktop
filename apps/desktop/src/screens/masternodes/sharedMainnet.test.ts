// MG's mainnet ruling in the rendered screens: every shared-masternode send button (Dissolve now,
// a standby's Send the full refund / Send now and pay the penalty / Send it, and a dissolution
// together's Send dissolution) shows on testnet and is absent on mainnet, where MG's lines stand.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { Backend } from "../../api/backend";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { SharedShares } from "../../api/generated/SharedShares";
import type { Standby } from "../../api/generated/Standby";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { BackendContext } from "../../api/queries";
import { confirmedCoordinator, sharedShares, SH_MN, standbyPair, togetherSession } from "../../fixture/shared";
import { t } from "../../i18n";
import { mnKeys } from "../../masternodes/queries";
import { DISSOLVE_FEE, shareLegend } from "../../masternodes/shared";
import { SharedSessionSheet } from "./Shared";
import { MaintenanceSheets, Standbys, type MaintenanceCtx } from "./SharedMaintenance";

vi.mock("../../app/ui", async (original) => ({
  ...(await original<typeof import("../../app/ui")>()),
  useUi: (select: (state: unknown) => unknown) => select({ discreet: false, prefs: { unit: "dash", decimalDigits: 8, language: null } }),
}));

// The session sheet's hand-off check reads SPV's state: running, with a peer.
vi.mock("../../api/queries", async (original) => ({
  ...(await original<typeof import("../../api/queries")>()),
  useSyncStatus: () => ({ data: { running: true, connectedPeers: 8 } }),
}));

const TESTNET: NetworkInfo = { kind: "testnet", dirName: "testnet", unitName: "tDASH" };
const MAINNET: NetworkInfo = { kind: "mainnet", dirName: "mainnet", unitName: "DASH" };
const W = "fixture";
const wallet = { walletId: W, watchOnly: false } as WalletSummary;
const COIN = 100_000_000;
const D = "masternodes.shared.detail.standby";
const SEND_LABELS = [t(`${D}.sendFull`), t(`${D}.sendImmediate`), t(`${D}.sendIt`), t("masternodes.shared.dissolve.button"), t("masternodes.shared.row.togetherSendButton")];

/** `text` as the server renderer escapes it. */
const esc = (text: string) => text.replace(/&/g, "&amp;").replace(/'/g, "&#x27;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

function render(el: ReactElement, seed: (q: QueryClient) => void = () => {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  seed(client);
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(BackendContext.Provider, { value: {} as Backend }, el)));
}

function ctx(network: NetworkInfo, shares: SharedShares, standbys: Standby[]): MaintenanceCtx {
  const session = confirmedCoordinator();
  return { wallet, network, hash: SH_MN, shares, legend: shareLegend(shares, session), session, standbys, onOpenSession: () => {} };
}

/** The standbys in each state with a send: immediate only (early), full (valid), and "Send it". */
const STANDBYS: [string, SharedShares, Standby[], string][] = [
  ["early", sharedShares({}), [standbyPair()], `${D}.sendImmediate`],
  ["full valid", sharedShares({ fullValidNow: true }), [standbyPair()], `${D}.sendFull`],
  ["only immediate held", sharedShares({ fullValidNow: true }), [standbyPair(undefined, { full: false })], `${D}.sendIt`],
];

const previewed = (c: MaintenanceCtx) => (q: QueryClient) =>
  q.setQueryData([...mnKeys.detail(W, c.hash), "previewDissolve", 0, DISSOLVE_FEE], {
    early: true,
    penalty: 20 * COIN,
    fee: DISSOLVE_FEE,
    validFromHeight: c.shares.fullValidFrom,
    returns: c.shares.shares.map((f) => ({ shareIndex: f.index, mine: f.mine, amount: f.amount })),
    warnings: [],
    totalDebit: 20 * COIN + DISSOLVE_FEE,
  });

const sessionSheet = (network: NetworkInfo) => {
  const s = togetherSession("participant", "combined");
  return render(createElement(SharedSessionSheet, { wallet, network, open: { id: s.id, imported: null }, onClose: () => {} }), (q) => q.setQueryData(mnKeys.sharedSession(W, s.id), s));
};

const noSendLabel = (html: string) => {
  for (const label of SEND_LABELS) expect(html, label).not.toContain(`>${esc(label)}<`);
};

describe("shared-masternode sends on mainnet", () => {
  it.each(STANDBYS)("a standby (%s) offers its send on testnet, none on mainnet", (_, shares, standbys, label) => {
    const on = render(createElement(Standbys, { c: ctx(TESTNET, shares, standbys), actions: true, onSheet: () => {} }));
    expect(on).toContain(`>${t(label as `${typeof D}.sendFull`)}<`);
    expect(on).toContain('data-testid="sh-standby-send-');
    const off = render(createElement(Standbys, { c: ctx(MAINNET, shares, standbys), actions: true, onSheet: () => {} }));
    expect(off).not.toContain('data-testid="sh-standby-send-');
    expect(off).toContain('data-testid="sh-standby-save-');
    noSendLabel(off);
  });

  it("Dissolve now shows on testnet; on mainnet MG's line stands in its place", () => {
    const on = ctx(TESTNET, sharedShares({}), [standbyPair()]);
    const testnet = render(createElement(MaintenanceSheets, { c: on, sheet: { kind: "dissolve", segment: "alone" }, onSheet: () => {} }), previewed(on));
    expect(testnet).toContain('data-testid="sh-dissolve-now"');
    const off = ctx(MAINNET, sharedShares({}), [standbyPair()]);
    for (const segment of ["alone", "standby"] as const) {
      const html = render(createElement(MaintenanceSheets, { c: off, sheet: { kind: "dissolve", segment }, onSheet: () => {} }), previewed(off));
      expect(html, segment).not.toContain('data-testid="sh-dissolve-now"');
      expect(html, segment).not.toContain('data-testid="sh-standby-send-');
      expect(html, segment).toContain(esc(t("masternodes.shared.mainnetDissolveNote")));
      noSendLabel(html);
    }
  });

  it("a combined dissolution together offers Send dissolution on testnet; on mainnet the note", () => {
    const testnet = sessionSheet(TESTNET);
    expect(testnet).toContain('data-testid="sh-send-maintenance"');
    expect(testnet).toContain(`>${t("masternodes.shared.row.togetherSendButton")}<`);
    const mainnet = sessionSheet(MAINNET);
    expect(mainnet).not.toContain('data-testid="sh-send-maintenance"');
    expect(mainnet).toContain(esc(t("masternodes.shared.mainnetSendNote")));
    noSendLabel(mainnet);
  });
});
