import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../api/backend";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { TxDetails } from "../api/generated/TxDetails";
import { BackendContext, keys } from "../api/queries";
import { ConfirmSheet } from "../screens/pay/ConfirmSheet";
import type { Advanced, Confirmed, Sending } from "../screens/pay/model";
import type { DraftApi, DraftSpec } from "../screens/pay/useDraft";
import { ReceivePage } from "../screens/Receive";
import { TxDetailSheet } from "../screens/TxDetailSheet";
import * as copyModule from "./copy";
import { CopyValue, Identifier, middleTruncate } from "./detail";
import { copyWithToast } from "./toast";

vi.mock("../payments/received", async (original) => ({
  ...(await original<typeof import("../payments/received")>()),
  useReceived: () => [],
  useLookupPending: () => false,
}));

const TXID = "184696c2a9b0c1d2e3f4a5b6c7d8e9f01234567890abcdef123456789f7e8be4";
const RECEIVE_ADDRESS = "yR1f5n8W9h2K4m7P3q6T8v1X4z7B2c5D8e";
const RECIPIENT_ADDRESS = "yS9g2k4M6n8P1q3R5t7V9w2X4y6Z8a1B3c";
const CHANGE_ADDRESS = "yC3d5e7F9g1H3j5K7m9N2p4Q6r8S1t3U5v";

const NETWORK: NetworkInfo = {
  kind: "testnet",
  dirName: "testnet",
  unitName: "tDASH",
};

function renderWithProviders(node: ReactNode, seed?: (client: QueryClient) => void) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  seed?.(client);
  return renderToStaticMarkup(
    createElement(QueryClientProvider, { client }, createElement(BackendContext.Provider, { value: {} as Backend }, node)),
  );
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("Identifier and CopyValue middle truncation", () => {
  it("puts the full value in title and accessible text while rendering one middle-truncated line", () => {
    expect(middleTruncate(TXID)).toBe("184696c2…f7e8be4");
    expect(middleTruncate("short-id-1234")).toBe("short-id-1234");

    const html = renderToStaticMarkup(createElement(Identifier, { value: TXID, mono: true }));
    expect(html).toContain(`title="${TXID}"`);
    expect(html).toContain('class="identifier mono"');
    expect(html).toContain('<span aria-hidden="true">184696c2…f7e8be4</span>');
    expect(html).toContain(`<span class="sr-only">${TXID}</span>`);

    const copyHtml = renderToStaticMarkup(createElement(CopyValue, { value: TXID, label: "transaction ID", mono: true, truncate: true }));
    expect(copyHtml).toContain(`title="${TXID}"`);
    expect(copyHtml).toContain('<span aria-hidden="true">184696c2…f7e8be4</span>');
    expect(copyHtml).toContain(`<span class="sr-only">${TXID}</span>`);
    expect(copyHtml).toContain('aria-label="Copy transaction ID"');
  });

  it("copies the full exact value when truncated", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });

    const copiedTexts: string[] = [];
    const OriginalCopyIconButton = copyModule.CopyIconButton;
    vi.spyOn(copyModule, "CopyIconButton").mockImplementation((props) => {
      copiedTexts.push(props.text);
      return OriginalCopyIconButton(props);
    });

    renderToStaticMarkup(createElement(CopyValue, { value: TXID, label: "transaction ID", mono: true, truncate: true }));

    expect(copiedTexts).toEqual([TXID]);
    expect(await copyWithToast(copiedTexts[0])).toBe(true);
    expect(writeText).toHaveBeenCalledExactlyOnceWith(TXID);
  });

  it("keeps the Pay confirm recipient and change addresses whole (never truncated)", () => {
    const confirmed: Confirmed = {
      recipient: { address: RECIPIENT_ADDRESS, amount: 100_000_000, subtractFeeFromAmount: false, label: null, message: null },
      more: [],
      estimate: {
        fee: 226,
        sizeBytes: 226,
        inputCount: 1,
        change: 50_000_000,
        totalSent: 100_000_000,
        leftOver: 0,
      },
      changeAddress: CHANGE_ADDRESS,
      total: 100_000_226,
    };
    const coins: Advanced = {
      coins: "any",
      selected: [],
      fee: { kind: "recommended", targetBlocks: 2 },
      changeAddress: CHANGE_ADDRESS,
    };
    const spec: DraftSpec = {
      recipient: confirmed.recipient,
      more: [],
      source: { kind: "any" },
      fee: { kind: "recommended", targetBlocks: 2 },
      change: { kind: "address", address: CHANGE_ADDRESS },
    };
    const draft: DraftApi = {
      estimate: vi.fn(),
      prepare: vi.fn(),
      broadcast: vi.fn(),
      abandon: vi.fn(),
      createUnsigned: vi.fn(),
      close: vi.fn(),
    };

    const html = renderWithProviders(
      createElement(ConfirmSheet, {
        confirmed,
        spec,
        coins,
        walletId: "w1",
        network: NETWORK,
        draft,
        returnFocus: () => null,
        onCancel: () => undefined,
        onOutcome: () => undefined,
        onSend: (_sending: Sending) => ({ broadcasting: vi.fn(), outcome: vi.fn(), notice: vi.fn(), release: vi.fn() }),
      }),
      (client) => {
        client.setQueryData(keys.wallets, [{ walletId: "w1", label: "Main", watchOnly: false }]);
        client.setQueryData(keys.lockStatus, { locked: false, retryAfterSecs: 0 });
        client.setQueryData(keys.paymentAuth, false);
      },
    );

    expect(html).toContain(`<span class="address-full">${RECIPIENT_ADDRESS}</span>`);
    expect(html).toContain(`<span class="address-full">${CHANGE_ADDRESS}</span>`);
    expect(html).not.toContain(middleTruncate(RECIPIENT_ADDRESS));
    expect(html).not.toContain(middleTruncate(CHANGE_ADDRESS));
    expect(html).not.toContain('class="identifier');
  });

  it("keeps the Receive screen address whole (never truncated)", () => {
    const html = renderWithProviders(createElement(ReceivePage, { walletId: "w1", network: NETWORK }), (client) => {
      client.setQueryData(keys.receiveAddress("w1"), { address: RECEIVE_ADDRESS, path: "m/44'/1'/0'/0/0", index: 0 });
    });

    expect(html).toContain(`<p class="address-full" data-testid="receive-address">${RECEIVE_ADDRESS}</p>`);
    expect(html).not.toContain(middleTruncate(RECEIVE_ADDRESS));
    expect(html).not.toContain('class="identifier');
  });

  it("lets Transaction details inputs and outputs copy their full addresses and outpoint", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });

    const copiedTexts: string[] = [];
    const OriginalCopyIconButton = copyModule.CopyIconButton;
    vi.spyOn(copyModule, "CopyIconButton").mockImplementation((props) => {
      copiedTexts.push(props.text);
      return OriginalCopyIconButton(props);
    });

    const PREV_TXID = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    const INPUT_ADDR = "yInputAddr11111111111111111111111111";
    const OUTPUT_ADDR = "yOutputAddr2222222222222222222222222";
    const details: TxDetails = {
      txid: TXID,
      rows: [
        {
          txid: TXID,
          recordIndex: 0,
          txType: "sendToAddress",
          category: "sent",
          status: { kind: "confirmed", confirmations: 6, instantLocked: false, chainLocked: false, maturesIn: null },
          timestamp: 1_700_000_000,
          blockHeight: 100,
          amount: -50_000_226,
          fee: 226,
          address: OUTPUT_ADDR,
          label: null,
          countsTowardBalance: true,
          involvesWatchOnly: false,
        },
      ],
      status: { kind: "confirmed", confirmations: 6, instantLocked: false, chainLocked: false, maturesIn: null },
      timestamp: 1_700_000_000,
      blockHeight: 100,
      blockHash: "00".repeat(32),
      fee: 226,
      sizeBytes: 226,
      inputs: [{ previousTxid: PREV_TXID, previousVout: 1, address: INPUT_ADDR, amount: 100_000_000, isMine: true }],
      outputs: [{ vout: 0, address: OUTPUT_ADDR, amount: 50_000_000, isMine: false, isChange: false, dataHex: null }],
      message: null,
      label: null,
      rawHex: "00".repeat(32),
    };

    const html = renderWithProviders(createElement(TxDetailSheet, { walletId: "w1", txid: TXID, network: NETWORK }), (client) => {
      client.setQueryData(keys.txDetail("w1", TXID), details);
    });

    expect(html).toContain(`title="${PREV_TXID}:1"`);
    expect(html).toContain(`title="${INPUT_ADDR}"`);
    expect(html).toContain(`title="${OUTPUT_ADDR}"`);
    expect(copiedTexts).toContain(`${PREV_TXID}:1`);
    expect(copiedTexts).toContain(INPUT_ADDR);
    expect(copiedTexts).toContain(OUTPUT_ADDR);

    expect(await copyWithToast(INPUT_ADDR)).toBe(true);
    expect(await copyWithToast(OUTPUT_ADDR)).toBe(true);
    expect(writeText).toHaveBeenNthCalledWith(1, INPUT_ADDR);
    expect(writeText).toHaveBeenNthCalledWith(2, OUTPUT_ADDR);
  });
});
