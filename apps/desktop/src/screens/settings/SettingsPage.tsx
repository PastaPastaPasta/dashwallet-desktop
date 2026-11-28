// Settings (U-04 D1): one route, seven tabs (CoinJoin: U-06; Advanced: DP6-01). Each tab is a page of cards; what the engine or the
// shell cannot do yet is listed with the task that brings it (D7).

import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { ui, useUi, type SettingsTab } from "../../app/ui";
import { useT } from "../../i18n";
import { Page, Tabs, type Tab } from "../Routes";
import { AboutTab } from "./About";
import { BackupsTab } from "./Backups";
import { CoinJoinTab } from "./CoinJoin";
import { GeneralTab } from "./General";
import { AdvancedTab } from "./Identities";
import { SecurityTab } from "./Security";
import { WalletsTab } from "./Wallets";

const SETTINGS_TABS: SettingsTab[] = ["general", "security", "wallets", "coinjoin", "backups", "advanced", "about"];

export function SettingsPage({ network }: { network: NetworkInfo }) {
  const t = useT();
  const tab = useUi((s) => s.settingsTab);
  const tabs: Tab<SettingsTab>[] = SETTINGS_TABS.map((id) => ({ id, title: t(`settings.tabs.${id}`) }));
  return (
    <Page route="settings">
      <Tabs label={t("routes.settings")} tabs={tabs} current={tab} onSelect={(settingsTab) => ui.set({ settingsTab })} testId="settings-tabs">
        {tab === "general" && <GeneralTab network={network} />}
        {tab === "security" && <SecurityTab />}
        {tab === "wallets" && <WalletsTab />}
        {tab === "coinjoin" && <CoinJoinTab network={network} />}
        {tab === "backups" && <BackupsTab />}
        {tab === "advanced" && <AdvancedTab network={network} />}
        {tab === "about" && <AboutTab />}
      </Tabs>
    </Page>
  );
}
