// Tools (U-05 D1): one route with U-01's eight tabs. A tab mounts only while it is the current one
// and is keyed by the window's wallet (U-03 D24): a switch drops the console's lines, a loaded
// PSBT and a selection. The tabs share one grant model and its passphrase sheet.

import { useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useBackend } from "../../api/queries";
import { navigate, useUi, type ToolsTab } from "../../app/ui";
import { GrantModel } from "../../tools/grant";
import { Page, Tabs, type Tab } from "../Routes";
import { AddressBookTab } from "./AddressBook";
import { CoinSelectionTab } from "./CoinSelection";
import { GrantSheet, NeedsWallet } from "./common";
import { ConsoleTab } from "./Console";
import { InformationTab } from "./Information";
import { PeersTab } from "./Peers";
import { PsbtTab } from "./Psbt";
import { RepairTab } from "./Repair";
import { SignVerifyTab } from "./SignVerify";

const TOOLS_TABS: Tab<ToolsTab>[] = [
  { id: "information", title: "Information" },
  { id: "console", title: "Console" },
  { id: "peers", title: "Peers" },
  { id: "repair", title: "Repair" },
  { id: "addressBook", title: "Address Book" },
  { id: "signVerify", title: "Sign/Verify" },
  { id: "psbt", title: "PSBT" },
  { id: "coinSelection", title: "Coin Selection" },
];

export function ToolsPage({ wallet, wallets, network }: { wallet: WalletSummary | null; wallets: WalletSummary[]; network: NetworkInfo }) {
  const b = useBackend();
  const tab = useUi((s) => s.toolsTab);
  const [grants] = useState(() => new GrantModel(b));
  const key = `${tab}:${wallet?.walletId ?? ""}`;
  let content;
  if (tab === "information") content = <InformationTab key={key} />;
  else if (tab === "console") content = <ConsoleTab key={key} grants={grants} wallets={wallets} />;
  else if (tab === "peers") content = <PeersTab key={key} />;
  // Without a wallet a PSBT still reads: "(But no wallet is loaded.)".
  else if (tab === "psbt") content = <PsbtTab key={key} grants={grants} walletId={wallet?.walletId ?? null} network={network} />;
  else if (!wallet) content = <NeedsWallet testId={`tools-${tab}-no-wallet`} />;
  else if (tab === "repair") content = <RepairTab key={key} grants={grants} wallet={wallet} />;
  else if (tab === "addressBook") content = <AddressBookTab key={key} walletId={wallet.walletId} network={network} />;
  else if (tab === "signVerify") content = <SignVerifyTab key={key} grants={grants} walletId={wallet.walletId} network={network} />;
  else content = <CoinSelectionTab key={key} walletId={wallet.walletId} network={network} />;
  return (
    // Coin Selection's and Peers' tables have dash-qt's columns (D22).
    <Page route="tools" wide={tab === "coinSelection" || tab === "peers"}>
      <Tabs label="Tools" tabs={TOOLS_TABS} current={tab} onSelect={(toolsTab) => navigate("tools", { toolsTab })} testId="tools-tabs">
        {content}
      </Tabs>
      <GrantSheet grants={grants} />
    </Page>
  );
}
