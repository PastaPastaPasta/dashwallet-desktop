// Settings ▸ General ▸ Privacy (DP3-03b, DASHPAY §5 F15): "Include my username in payment
// requests" for the window's wallet. The engine keeps it per wallet, off by default (DP3-02 D21).
// "Load contact pictures" (DP4-03 D14) is the network's, on by default (DP4-02 D17).

import { useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { keys, useBackend, useUsernameInRequests, useWallets } from "../../api/queries";
import { changingUsernameInRequests } from "../../api/username";
import { useUi } from "../../app/ui";
import { useContactPictures, useSetContactPictures } from "../../dashpay/profileQueries";
import { useT } from "../../i18n";
import { failureText } from "../../onboarding/model";
import { Hint, Problem, SettingsCard } from "./parts";

/** The card of the wallet the window shows, picked as the shell picks it (App.tsx). */
export function PrivacyCard() {
  const t = useT();
  const walletId = useUi((s) => s.walletId);
  const wallets = useWallets();
  const wallet = wallets.data?.find((w) => w.walletId === walletId) ?? wallets.data?.[0];
  if (!wallet) return null;
  return (
    <SettingsCard title={t("settings.privacy.title")} testId="settings-privacy">
      <UsernameInRequests key={wallet.walletId} wallet={wallet} />
      <ContactPictures />
    </SettingsCard>
  );
}

/** D14: off, no other user's picture is read or fetched, and every list shows initials. */
function ContactPictures() {
  const t = useT();
  const setting = useContactPictures();
  const write = useSetContactPictures();
  const [turning, setTurning] = useState<boolean | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const hint = useId();
  if (setting.isPending) return null;
  const turn = async (on: boolean) => {
    setProblem(null);
    setTurning(on);
    try {
      await write(on);
    } catch (e) {
      setProblem(failureText(e));
    }
    setTurning(null);
  };
  return (
    <>
      {setting.isError && <Problem text={failureText(setting.error)} testId="settings-pictures-error" />}
      <label className="checkbox-row">
        <input
          type="checkbox"
          data-testid="settings-contact-pictures"
          checked={turning ?? setting.data === true}
          disabled={turning !== null}
          aria-describedby={hint}
          onChange={(e) => void turn(e.target.checked)}
        />
        {t("settings.privacy.contactPictures")}
      </label>
      <Hint id={hint}>{t("settings.privacy.contactPicturesHint")}</Hint>
      <Problem text={problem} testId="settings-pictures-problem" />
    </>
  );
}

function UsernameInRequests({ wallet }: { wallet: WalletSummary }) {
  const t = useT();
  const backend = useBackend();
  const client = useQueryClient();
  const setting = useUsernameInRequests(wallet.walletId);
  /** The box as clicked, until the engine has answered (U-04 D36). */
  const [turning, setTurning] = useState<boolean | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const hint = useId();
  if (setting.isPending) return null;
  const turn = async (on: boolean) => {
    setProblem(null);
    setTurning(on);
    // While the change is under way the setting reads off, either way: Receive, opened
    // meanwhile, shows no name (D7, D22). A read already under way would put the old value
    // back, so it is dropped first.
    const done = changingUsernameInRequests(wallet.walletId);
    await client.cancelQueries({ queryKey: keys.usernameInRequests(wallet.walletId) });
    client.setQueryData(keys.usernameInRequests(wallet.walletId), false);
    try {
      await backend.setUsernameInRequests(wallet.walletId, on);
    } catch (e) {
      setProblem(failureText(e));
    } finally {
      done();
    }
    // The Receive QR's username, the requests and the setting are all receive reads. Receive is
    // not mounted here, so its cached reads are refetched too, not only marked stale: a cached
    // name must not show while Receive refetches on its way back. After a refusal this puts the
    // engine's value back.
    await client.invalidateQueries({ queryKey: keys.receiveAll(wallet.walletId), refetchType: "all" });
    setTurning(null);
  };
  return (
    <>
      {/* Unreadable, the requests carry no name (D6), so the box shows off; a click writes a
          value the engine reads again. */}
      {setting.isError && <Problem text={failureText(setting.error)} testId="settings-username-error" />}
      <label className="checkbox-row">
        <input
          type="checkbox"
          data-testid="settings-username-in-requests"
          checked={turning ?? (!setting.isError && setting.data === true)}
          // One save at a time: a second click's answer could arrive first (U-04 D36).
          disabled={turning !== null}
          aria-describedby={hint}
          onChange={(e) => void turn(e.target.checked)}
        />
        {t("settings.privacy.usernameInRequests")}
      </label>
      <Hint id={hint}>{t("settings.privacy.usernameInRequestsHint", { wallet: wallet.name })}</Hint>
      <Problem text={problem} testId="settings-username-problem" />
    </>
  );
}
