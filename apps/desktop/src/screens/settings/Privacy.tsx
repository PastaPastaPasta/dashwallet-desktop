// Settings ▸ General ▸ Privacy (DP3-03b, DASHPAY §5 F15): "Include my username in payment
// requests" for the window's wallet. The engine keeps it per wallet, on by default (DP3-02 D21).

import { useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { keys, useBackend, useUsernameInRequests, useWallets } from "../../api/queries";
import { useUi } from "../../app/ui";
import { failureText } from "../../onboarding/model";
import { Hint, Problem, SettingsCard } from "./parts";

/** The card of the wallet the window shows, picked as the shell picks it (App.tsx). */
export function PrivacyCard() {
  const walletId = useUi((s) => s.walletId);
  const wallets = useWallets();
  const wallet = wallets.data?.find((w) => w.walletId === walletId) ?? wallets.data?.[0];
  if (!wallet) return null;
  return <UsernameInRequests key={wallet.walletId} wallet={wallet} />;
}

function UsernameInRequests({ wallet }: { wallet: WalletSummary }) {
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
    // Off counts at once, before the engine answers: Receive, opened meanwhile, must not draw a
    // cached name (D7). A read already under way would put the old value back, so it is
    // dropped first. On waits for the engine.
    if (!on) {
      await client.cancelQueries({ queryKey: keys.usernameInRequests(wallet.walletId) });
      client.setQueryData(keys.usernameInRequests(wallet.walletId), false);
    }
    try {
      await backend.setUsernameInRequests(wallet.walletId, on);
    } catch (e) {
      setProblem(failureText(e));
    }
    // The Receive QR's username, the requests and the setting are all receive reads. Receive is
    // not mounted here, so its cached reads are refetched too, not only marked stale: a cached
    // name must not show while Receive refetches on its way back. After a refusal this puts the
    // engine's value back.
    await client.invalidateQueries({ queryKey: keys.receiveAll(wallet.walletId), refetchType: "all" });
    setTurning(null);
  };
  return (
    <SettingsCard title="Privacy" testId="settings-privacy">
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
        Include my username in payment requests
      </label>
      <Hint id={hint}>
        The Receive QR code and the payment requests of “{wallet.name}” show its DashPay username, once it has one, so whoever sees them knows the Dash
        address is yours. Turned off, they hold only the address and what you enter.
      </Hint>
      <Problem text={problem} testId="settings-username-problem" />
    </SettingsCard>
  );
}
