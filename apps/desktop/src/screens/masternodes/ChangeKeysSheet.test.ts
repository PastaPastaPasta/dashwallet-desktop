import { createElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import type { Backend } from "../../api/backend";
import type { MasternodeDetail } from "../../api/generated/MasternodeDetail";
import type { MasternodeRow } from "../../api/generated/MasternodeRow";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { ProTxOperation } from "../../api/generated/ProTxOperation";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { BackendContext } from "../../api/queries";
import { t } from "../../i18n";
import { registrarUpdateSends } from "../../masternodes/changeKeys";
import { ChangeKeysSheet } from "./ChangeKeysSheet";

const HASH = "4e".repeat(32);
const ops = vi.hoisted(() => ({ current: [] as unknown[] }));
const credentialError = vi.hoisted(() => ({ current: null as string | null }));

vi.mock("../../masternodes/queries", async (original) => ({
  ...(await original<typeof import("../../masternodes/queries")>()),
  useMasternodeOperations: () => ({ data: ops.current, isSuccess: true }),
  useMasternodesRefresh: () => () => undefined,
}));
vi.mock("./credential", () => ({
  useCredential: () => ({ ready: true, error: credentialError.current, authorize: async () => null, setError: () => undefined }),
  CredentialField: () => createElement("i", { "data-testid": "credential" }),
}));
vi.mock("../../components/sheet", () => ({
  Sheet: ({ children }: { children: ReactNode }) => createElement("section", null, children),
}));

const unsent = { id: "op1", kind: "update_registrar", proTxHash: HASH, state: { kind: "not_sent" }, lastError: null, debit: 1000 } as unknown as ProTxOperation;

const render = (operations: ProTxOperation[], kind: NetworkInfo["kind"] = "mainnet") => {
  ops.current = operations;
  const sheet = createElement(ChangeKeysSheet, {
    wallet: { walletId: "w" } as WalletSummary,
    network: { kind, dirName: kind, unitName: kind === "mainnet" ? "DASH" : "tDASH" } as NetworkInfo,
    row: { proTxHash: HASH, kind: "regular" } as unknown as MasternodeRow,
    detail: { registration: { payoutAddress: "yRecordedPayoutAddressMn7qR2vWx9Kf" }, list: null } as unknown as MasternodeDetail,
    onClose: () => undefined,
  });
  const client = new QueryClient();
  return renderToStaticMarkup(
    createElement(QueryClientProvider, { client }, createElement(BackendContext.Provider, { value: {} as Backend }, sheet)),
  );
};

describe("Change keys or payout follows the network", () => {
  it("is off on mainnet and while the network is still loading, and on for testnet, regtest and devnet", () => {
    expect(registrarUpdateSends(null)).toBe(false);
    expect(registrarUpdateSends(undefined)).toBe(false);
    expect(registrarUpdateSends({ kind: "mainnet" })).toBe(false);
    expect(registrarUpdateSends({ kind: "testnet" })).toBe(true);
    expect(registrarUpdateSends({ kind: "regtest" })).toBe(true);
    expect(registrarUpdateSends({ kind: "devnet" })).toBe(true);
  });

  it("offers no Send or passphrase on mainnet, and says nothing has been sent", () => {
    const html = render([]);
    expect(html).toContain('data-testid="mn-change-operator"');
    expect(html).not.toContain("mn-change-send");
    expect(html).not.toContain('data-testid="credential"');
    expect(html).toContain('data-testid="mn-change-held"');
    expect(html).toContain(t("masternodes.myMasternodes.changeKeys.held").replaceAll("'", "&#x27;"));
  });

  it("offers Send and a passphrase on testnet", () => {
    const html = render([], "testnet");
    expect(html).toContain('data-testid="mn-change-send"');
    expect(html).toContain('data-testid="credential"');
    expect(html).not.toContain("mn-change-held");
  });

  it("offers Discard but no Send again for an update that didn't go out, on mainnet", () => {
    const html = render([unsent]);
    expect(html).toContain('data-testid="mn-change-unsent-discard"');
    expect(html).not.toContain("mn-change-unsent-resend");
    expect(html).not.toContain('data-testid="credential"');
    expect(html).toContain('data-testid="mn-change-held"');
  });

  it("offers Send again on testnet for an update that didn't go out", () => {
    const html = render([unsent], "testnet");
    expect(html).toContain('data-testid="mn-change-unsent-discard"');
    expect(html).toContain('data-testid="mn-change-unsent-resend"');
    expect(html).toContain('data-testid="credential"');
    expect(html).not.toContain("mn-change-held");
  });

  it("names a failed Discard where the passphrase field's error used to show", () => {
    try {
      credentialError.current = "The change couldn't be discarded.";
      expect(render([unsent])).toMatch(/role="alert" data-testid="mn-change-unsent-error">The change couldn&#x27;t be discarded\.</);
    } finally {
      credentialError.current = null;
    }
  });
});
