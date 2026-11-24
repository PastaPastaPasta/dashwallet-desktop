import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import type { Backend } from "../../api/backend";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { ProTxOperation } from "../../api/generated/ProTxOperation";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { BackendContext } from "../../api/queries";
import { InProgressGroup } from "./InProgress";

const op = (id: string, kind: ProTxOperation["kind"]): ProTxOperation =>
  ({
    id,
    kind,
    state: { kind: "sent" },
    lastError: null,
    overdue: false,
    txid: "aa".repeat(32),
    proTxHash: "bb".repeat(32),
  }) as unknown as ProTxOperation;

const render = (kind: NetworkInfo["kind"]) => {
  const group = createElement(InProgressGroup, {
    ops: [op("reg", "registration"), op("upd", "update_registrar")],
    wallet: { walletId: "w", watchOnly: false } as WalletSummary,
    network: { kind, dirName: kind, unitName: kind === "mainnet" ? "DASH" : "tDASH" } as NetworkInfo,
    onContinue: () => undefined,
  });
  const client = new QueryClient();
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(BackendContext.Provider, { value: {} as Backend }, group)));
};

describe("In progress Send again follows the network", () => {
  it("hides Send again for a registration and a registrar update on mainnet", () => {
    const html = render("mainnet");
    expect(html).not.toContain("mn-op-resend-reg");
    expect(html).not.toContain("mn-op-resend-upd");
  });

  it("shows Send again for a registration and a registrar update on testnet", () => {
    const html = render("testnet");
    expect(html).toContain('data-testid="mn-op-resend-reg"');
    expect(html).toContain('data-testid="mn-op-resend-upd"');
  });
});
