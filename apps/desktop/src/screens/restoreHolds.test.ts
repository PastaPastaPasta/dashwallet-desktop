// The R3 restore holds in the renderer (Q92 H1, H2 and the RESTORED money gate): a held coin is shown
// with MG's pendingUnproven line but never offered and never counted as spendable; restoredHint shows
// above the abandon confirm only while the payment holds its hold; while RESTORED is set the money
// gate's line stands in for Abandon, and the engine refuses it.

import { clearMocks } from "@tauri-apps/api/mocks";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../api/backend";
import * as cmd from "../api/generated/commands";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { TxDetailExtrasInfo } from "../api/generated/TxDetailExtrasInfo";
import type { Utxo } from "../api/generated/Utxo";
import { BackendContext, keys } from "../api/queries";
import { pendingUnproven, restored, restoreHeld, type HeldTxExtras, type HeldUtxo, type RestoredNetworkInfo } from "../api/restoreHolds";
import { CoinTable } from "../components/coins";
import { display } from "../format/display";
import { coreSubtitle } from "../format/homeBalance";
import { installFixture } from "../fixture/index";
import { FIXTURE_WALLET_ID as W, RESTORE_HELD_TXID as P } from "../fixture/scenarios";
import { DEFAULT_PREFS, ui } from "../app/ui";
import { t } from "../i18n";
import { CoinSelectionModel } from "../tools/coins";
import { notSelectable, outpointKey } from "./pay/model";
import { abandonHint, actionOffer, actionProblem, Actions } from "./TxDetailSheet";

const PENDING_UNPROVEN =
  "Dash Wallet can't yet tell whether the payment that sent this coin will confirm, so the coin isn't counted in your spendable balance or offered for spending until it does.";
const RESTORED_HINT =
  "This payment was still pending in your wallet data from before the upgrade, and it hasn't confirmed since. Anything you did with it after the upgrade, such as abandoning it, is no longer recorded here.";
const ALL_COINS = { includeLocked: true, fullyMixedOnly: false, minConfirmations: null };
const SHOWN = display({ unit: "dash", decimalDigits: 2 }, { kind: "testnet", dirName: "testnet", unitName: "tDASH" });
const escaped = (text: string) => text.replace(/&/g, "&amp;").replace(/'/g, "&#x27;").replace(/"/g, "&quot;");
const coinRow = (html: string, { outpoint }: Utxo) => new RegExp(`<tr data-testid="coin-${outpoint.txid.slice(0, 8)}-${outpoint.vout}"[^>]*>(.*?)</tr>`).exec(html)?.[1] ?? "";

beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  ui.set({ theme: null, prefs: DEFAULT_PREFS });
  clearMocks();
  vi.unstubAllGlobals();
});

const coinsOf = () => cmd.utxos(W, ALL_COINS);
const heldCoin = async () => (await coinsOf()).find((u) => u.outpoint.txid === P)!;

describe("a coin a restored payment still holds (H1)", () => {
  it("is shown as Pending with MG's line as its tooltip and description, its checkbox off, in both coin tables and themes", async () => {
    installFixture({ scenario: "restore-holds" });
    const coins = await coinsOf();
    const held = coins.find((u) => u.outpoint.txid === P);
    expect(held).toBeDefined();
    expect(pendingUnproven(held!)).toBe(true);
    expect(notSelectable(held!)).toBe("Pending");
    const DETAIL = { sort: { column: "amount" as const, ascending: false }, onSort: () => {}, groups: null, onToggleGroup: () => {}, actions: () => null };
    for (const detail of [undefined, DETAIL]) {
      const render = () => renderToStaticMarkup(createElement(CoinTable, { coins, shown: SHOWN, selected: new Set<string>(), onToggle: () => {}, detail }));
      const [light, dark] = (["light", "dark"] as const).map((theme) => {
        ui.set({ theme });
        return render();
      });
      expect(dark).toBe(light);
      const row = coinRow(light, held!);
      const id = /aria-describedby="([^"]+)"/.exec(row)?.[1];
      expect(id).toBeDefined();
      expect(row).toMatch(/<input type="checkbox"[^>]*disabled=""/);
      expect(row).toContain(`<span id="${id}" hidden="">${escaped(PENDING_UNPROVEN)}</span>`);
      expect(row).toContain(`<td title="${escaped(PENDING_UNPROVEN)}">Pending</td>`);
      for (const other of coins.filter((u) => u.outpoint.txid !== P)) {
        const plain = coinRow(light, other);
        expect(plain).not.toContain("aria-describedby");
        expect(plain).not.toContain(escaped(PENDING_UNPROVEN));
      }
    }
  });

  it("is never offered, even if the engine also called it spendable", async () => {
    installFixture({ scenario: "restore-holds" });
    const held = await heldCoin();
    const key = outpointKey(held.outpoint);
    const model = new CoinSelectionModel({ lockOutpoints: async () => null, unlockOutpoints: async () => null, coinSelectionSummary: async () => { throw new Error("no summary"); } } as never, W, coinsOf);
    model.setCoins([...(await coinsOf()).filter((u) => u !== held), { ...held, spendable: true } satisfies HeldUtxo]);
    model.toggle(key);
    expect(model.selected).not.toContain(key);
    model.toggleAll();
    expect(model.selected.length).toBeGreaterThan(0);
    expect(model.selected).not.toContain(key);
    expect(notSelectable({ ...held, spendable: true })).toBe("Pending");
  });

  it("counts in Home's pending figure, never in the spendable one", async () => {
    installFixture({ scenario: "funded" });
    const before = (await cmd.balance(W))!;
    clearMocks();
    installFixture({ scenario: "restore-holds" });
    const held = await heldCoin();
    const b = (await cmd.balance(W))!;
    expect(b.unconfirmed).toBe(before.unconfirmed + held.amount);
    const spendable = (await coinsOf()).filter((u) => notSelectable(u) === null).reduce((n, u) => n + u.amount, 0);
    expect(b.confirmed).toBe(spendable);
    expect(coreSubtitle(b, (d) => String(d), "Dash wallet")).toBe(t("home.core.pending", { amount: String(before.unconfirmed + held.amount) }));
  });

  it("a coin without the flag reads as no hold", () => {
    const plain = { spendable: true, reserved: false, userLocked: false, isCoinbase: false } as Utxo;
    expect(pendingUnproven(plain)).toBe(false);
    expect(notSelectable(plain)).toBeNull();
  });

  it("goes with the hold when the payment is abandoned", async () => {
    installFixture({ scenario: "funded" });
    const funded = (await cmd.balance(W))!;
    clearMocks();
    installFixture({ scenario: "restore-holds" });
    await cmd.abandonTransaction(W, P);
    expect((await coinsOf()).filter(pendingUnproven)).toEqual([]);
    expect(await cmd.balance(W)).toMatchObject({ confirmed: funded.confirmed, unconfirmed: funded.unconfirmed, total: funded.total });
  });
});

describe("the abandon confirm of a payment from before a restore (H2)", () => {
  const extras = (over: Partial<HeldTxExtras>): HeldTxExtras => ({ canAbandon: true, totalDebit: 1, inputsUnresolved: false, ...over }) as HeldTxExtras;

  it("starts with MG's restoredHint only while the payment holds its hold", () => {
    expect(abandonHint(extras({ restoreHeld: true }))).toBe(RESTORED_HINT);
    expect(abandonHint(extras({ restoreHeld: false }))).toBeNull();
    expect(abandonHint(extras({}))).toBeNull();
  });

  it("is told by the engine which payment holds it, and the hold ends with the abandon", async () => {
    installFixture({ scenario: "restore-holds" });
    const x = await cmd.txDetailExtras(W, P);
    expect(restoreHeld(x)).toBe(true);
    expect(abandonHint(x)).toBe(RESTORED_HINT);
    const other = (await cmd.historyPage(W, { categories: [], text: null, sort: "newestFirst", cursor: null, limit: 5 })).rows.find((r) => r.txid !== P)!;
    expect(abandonHint(await cmd.txDetailExtras(W, other.txid))).toBeNull();
    await cmd.abandonTransaction(W, P);
    expect(restoreHeld(await cmd.txDetailExtras(W, P))).toBe(false);
  });
});

describe("the RESTORED money gate", () => {
  const actions = (x: TxDetailExtrasInfo, network: NetworkInfo) => {
    const client = new QueryClient();
    client.setQueryData(keys.txDetailExtras(W, P), x);
    return renderToStaticMarkup(
      createElement(BackendContext.Provider, { value: {} as Backend }, createElement(QueryClientProvider, { client }, createElement(Actions, { walletId: W, txid: P, network }))),
    );
  };

  it("puts its line where Abandon was, and the engine refuses Abandon", async () => {
    installFixture({ scenario: "restore-holds-restored" });
    const n = (await cmd.activeNetwork())!;
    const x = await cmd.txDetailExtras(W, P);
    expect(restored(n)).toBe(true);
    expect(actionOffer(x.canAbandon, restored(n))).toBe("gated");
    expect(actionOffer(x.canResend, restored(n))).toBe("gated");
    const html = actions(x, n);
    expect(html).toContain(escaped(t("activity.abandon.restoredGate")));
    expect(html).not.toContain('data-testid="tx-abandon"');
    expect(html).not.toContain('data-testid="tx-resend"');
    const refusal = await cmd.abandonTransaction(W, P).catch((e: unknown) => e);
    expect(refusal).toMatchObject({ code: "tx_action_refused", data: { kind: "txAction", refusal: "restored" } });
    expect(actionProblem(refusal)).toBe(t("activity.abandon.restoredGate"));
    await expect(cmd.resendTransaction(W, P)).rejects.toMatchObject({ data: { kind: "txAction", refusal: "restored" } });
    expect((await coinsOf()).some(pendingUnproven)).toBe(true);
  });

  it("offers Abandon once RESTORED is cleared, the hold still there", async () => {
    installFixture({ scenario: "restore-holds" });
    const n = (await cmd.activeNetwork())!;
    const x = await cmd.txDetailExtras(W, P);
    expect(restored(n)).toBe(false);
    expect(actionOffer(x.canAbandon, restored(n))).toBe("offer");
    const html = actions(x, n);
    expect(html).toContain('data-testid="tx-abandon"');
    expect(html).toContain('data-testid="tx-resend"');
    expect(html).not.toContain("tx-restored-gate");
    await cmd.abandonTransaction(W, P);
  });

  it("reads a network answer without the flag as not restored, and Balances never gates", async () => {
    installFixture({ scenario: "restore-holds-restored" });
    expect(restored({ kind: "testnet", dirName: "testnet", unitName: "tDASH" })).toBe(false);
    expect(restored(null)).toBe(false);
    expect(Object.keys((await cmd.balance(W))!)).not.toContain("restored");
  });

  it("offers nothing for an action the engine says doesn't apply", () => {
    expect(actionOffer(false, true)).toBe("none");
    expect(actionOffer(false, false)).toBe("none");
  });

  it("names the funds pause for a payment that can only be resent", () => {
    const x = { canAbandon: false, canResend: true, totalDebit: 1, inputsUnresolved: false } as TxDetailExtrasInfo;
    const n: RestoredNetworkInfo = { kind: "testnet", dirName: "testnet", unitName: "tDASH", restored: true };
    const html = actions(x, n);
    expect(html).toContain(escaped(t("common.fundsPaused")));
    expect(html).not.toContain('data-testid="tx-resend"');
  });
});
