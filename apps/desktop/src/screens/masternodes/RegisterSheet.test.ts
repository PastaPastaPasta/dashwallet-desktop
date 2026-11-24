import { createElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import type { Backend } from "../../api/backend";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { ProTxOperation } from "../../api/generated/ProTxOperation";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { BackendContext } from "../../api/queries";
import { HELD_CALLS, SENDING_CALLS } from "../../masternodes/registration";
import { RegisterSheet } from "./RegisterSheet";

vi.mock("../../masternodes/queries", () => ({
  useMasternodes: () => ({ data: { collateralCoins: [] } }),
  useMasternodesRefresh: () => () => undefined,
  useRegistrationKeys: () => ({ isSuccess: false, isError: false, isPending: false, data: undefined }),
}));
vi.mock("./credential", () => ({
  useCredential: () => ({ ready: false, error: null, authorize: async () => null, setError: () => undefined }),
  CredentialField: () => createElement("i", { "data-testid": "reg-passphrase" }),
}));
vi.mock("../../components/sheet", () => ({
  Sheet: ({ children }: { children: ReactNode }) => createElement("section", null, children),
}));

const resume = {
  id: "op1",
  kind: "registration",
  state: { kind: "needs_signature" },
  debit: 1000,
  terms: { masternodeType: "regular", collateralOutpoint: "ab".repeat(32) + "-0" },
  signRequest: { address: "yCollateralAddressMn7qR2vWx9Kf", message: "sign-me" },
} as unknown as ProTxOperation;

const render = (kind: NetworkInfo["kind"]) => {
  const sheet = createElement(RegisterSheet, {
    wallet: { walletId: "w" } as WalletSummary,
    network: { kind, dirName: kind, unitName: kind === "mainnet" ? "DASH" : "tDASH" } as NetworkInfo,
    resume,
    onClose: () => undefined,
  });
  const client = new QueryClient();
  const backend = Object.fromEntries([...HELD_CALLS, ...SENDING_CALLS].map((k) => [k, () => Promise.resolve(null)])) as unknown as Backend;
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(BackendContext.Provider, { value: backend }, sheet)));
};

describe("Register on Prove follows the network", () => {
  it("hides Register on mainnet and shows the send-off line", () => {
    const html = render("mainnet");
    expect(html).toContain('data-testid="reg-send-off"');
    expect(html).not.toContain("reg-submit");
    expect(html).not.toContain("reg-register");
    expect(html).not.toContain('data-testid="reg-passphrase"');
  });

  it("shows Register on testnet", () => {
    const html = render("testnet");
    expect(html).toContain('data-testid="reg-submit"');
    expect(html).toContain('data-testid="reg-passphrase"');
    expect(html).not.toContain("reg-send-off");
  });
});
