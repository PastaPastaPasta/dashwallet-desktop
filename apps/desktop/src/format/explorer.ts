// Insight transaction pages, the same URLs as
// `TransactionsViewModel.explorerLinks` (WalletFeatures). Regtest and devnet
// are not on Insight, so they have no link.

import type { NetworkKind } from "../api/generated/NetworkKind";

const TXID = /^[0-9a-f]{64}$/;

/** The explorer page for `txid` on `kind`, or null when this network has none. */
export function explorerUrl(kind: NetworkKind, txid: string): string | null {
  if (!TXID.test(txid)) return null;
  switch (kind) {
    case "mainnet":
      return `https://insight.dash.org/insight/tx/${txid}`;
    case "testnet":
      return `https://insight.testnet.networks.dash.org:3002/insight/tx/${txid}`;
    case "regtest":
    case "devnet":
      return null;
  }
}
