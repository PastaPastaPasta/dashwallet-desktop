// Settings (U-04 D1): one route, six tabs (CoinJoin: U-06). Each tab is a page of cards; what the engine or the
// shell cannot do yet is listed with the task that brings it (D7).

import { ui, useUi, type SettingsTab } from "../../app/ui";
import { Page, Tabs, type Tab } from "../Routes";
import { AboutTab } from "./About";
import { BackupsTab } from "./Backups";
import { CoinJoinTab } from "./CoinJoin";
import { GeneralTab } from "./General";
import { SecurityTab } from "./Security";
import { WalletsTab } from "./Wallets";

const SETTINGS_TABS: Tab<SettingsTab>[] = [
  { id: "general", title: "General" },
  { id: "security", title: "Security" },
  { id: "wallets", title: "Wallets" },
  { id: "coinjoin", title: "CoinJoin" },
  { id: "backups", title: "Backups" },
  { id: "about", title: "About" },
];

export function SettingsPage() {
  const tab = useUi((s) => s.settingsTab);
  return (
    <Page route="settings">
      <Tabs label="Settings" tabs={SETTINGS_TABS} current={tab} onSelect={(settingsTab) => ui.set({ settingsTab })} testId="settings-tabs">
        {tab === "general" && <GeneralTab />}
        {tab === "security" && <SecurityTab />}
        {tab === "wallets" && <WalletsTab />}
        {tab === "coinjoin" && <CoinJoinTab />}
        {tab === "backups" && <BackupsTab />}
        {tab === "about" && <AboutTab />}
      </Tabs>
    </Page>
  );
}
