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
import { GrantSheet, NeedsWallet } from "./common";
import { ConsoleTab } from "./Console";
import { InformationTab } from "./Information";
import { PeersTab } from "./Peers";
import { RepairTab } from "./Repair";

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

export function ToolsPage({ wallet, wallets }: { wallet: WalletSummary | null; wallets: WalletSummary[]; network: NetworkInfo }) {
  const b = useBackend();
  const tab = useUi((s) => s.toolsTab);
  const [grants] = useState(() => new GrantModel(b));
  const key = `${tab}:${wallet?.walletId ?? ""}`;
  let content;
  if (tab === "information") content = <InformationTab key={key} />;
  else if (tab === "console") content = <ConsoleTab key={key} grants={grants} wallets={wallets} />;
  else if (tab === "peers") content = <PeersTab key={key} />;
  else if (!wallet) content = <NeedsWallet testId={`tools-${tab}-no-wallet`} />;
  else if (tab === "repair") content = <RepairTab key={key} grants={grants} wallet={wallet} />;
  else {
    const title = TOOLS_TABS.find((t) => t.id === tab)!.title;
    content = (
      <p className="placeholder card-l1" data-testid={`placeholder-tools-${tab}`}>
        {title} is not available yet.
      </p>
    );
  }
  return (
    <Page route="tools">
      <Tabs label="Tools" tabs={TOOLS_TABS} current={tab} onSelect={(toolsTab) => navigate("tools", { toolsTab })} testId="tools-tabs">
        {content}
      </Tabs>
      <GrantSheet grants={grants} />
    </Page>
  );
}
