// The one-time price disclosure (MP-01, rates-engine §1): no request is made before it was seen. A
// new wallet sees it on onboarding's last step; a 0.1 install that opens Mainnet sees it once as a
// Home card. Either way the answer is saved with `disclosed`, so neither shows again. Test networks
// never fetch, so neither shows there.

import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { RatesSettings } from "../api/generated/RatesSettings";
import { keys, useBackend } from "../api/queries";
import { DashButton } from "../components/basics";
import { currencyName, localeCurrency } from "../format/fiat";
import { RATES_DISCLOSURE, useRates } from "./rates";

/** The disclosure is still owed: Mainnet, prices on, and never seen. */
export function useRatesConsent(network: NetworkInfo) {
  const b = useBackend();
  const client = useQueryClient();
  const view = useRates().data;
  const settings = view?.settings;
  const needed = network.kind === "mainnet" && !!settings && settings.enabled && !settings.disclosed;
  const currency = settings?.currency ?? localeCurrency(b.locale);
  /** Saves the answer; a failed save leaves prices off (nothing was disclosed), so it is not shown. */
  const answer = async (on: boolean) => {
    if (!settings) return;
    const next: RatesSettings = { enabled: on, disclosed: true, currency: on ? currency : settings.currency };
    await b.setRatesSettings(next).catch(() => undefined);
    await client.invalidateQueries({ queryKey: keys.ratesAll });
  };
  return { needed, currency, name: currencyName(currency, b.locale), answer };
}

export type RatesConsent = ReturnType<typeof useRatesConsent>;

/** Onboarding's last step: the switch, on, with the disclosure under it; "Open wallet" saves it. */
export function OnboardingRatesChoice({ consent, on, setOn }: { consent: RatesConsent; on: boolean; setOn: (on: boolean) => void }) {
  return (
    <div className="onboarding-rates" data-testid="onboarding-rates">
      <label className="checkbox-row">
        <input type="checkbox" data-testid="onboarding-rates-on" checked={on} onChange={(e) => setOn(e.target.checked)} />
        Show values in {consent.name} ({consent.currency})
      </label>
      <p className="field-hint">{RATES_DISCLOSURE} You can change this in Settings ▸ General.</p>
    </div>
  );
}

/** Home's one-time card for a 0.1 install on Mainnet. */
export function RatesConsentCard({ network }: { network: NetworkInfo }) {
  const consent = useRatesConsent(network);
  const [busy, setBusy] = useState(false);
  if (!consent.needed) return null;
  const answer = (on: boolean) => {
    setBusy(true);
    void consent.answer(on).finally(() => setBusy(false));
  };
  return (
    <section className="rates-consent-card card-l1" data-testid="rates-consent-card" aria-label="Prices in local currency">
      <div className="platform-card-body">
        <h2 className="platform-card-title">Show prices in {consent.name}?</h2>
        <p className="platform-card-text">
          The wallet fetches exchange rates from CTX to show what amounts are worth in {consent.currency}. {RATES_DISCLOSURE}
        </p>
      </div>
      <div className="shielded-card-actions">
        <DashButton variant="tintedGray" size="small" data-testid="rates-consent-off" disabled={busy} onClick={() => answer(false)}>
          Turn off
        </DashButton>
        <DashButton variant="filledBlue" size="small" data-testid="rates-consent-ok" disabled={busy} onClick={() => answer(true)}>
          OK
        </DashButton>
      </div>
    </section>
  );
}
