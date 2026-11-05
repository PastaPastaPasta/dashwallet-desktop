// Settings ▸ General ▸ Local currency (MP-01, rates-engine §1–2): the one switch, the currency
// picker (CTX's local currencies with their names and prices) and the price status with
// "Update now". Turning the switch on here counts as seeing the disclosure, which is its hint.

import { useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import type { CurrencyRate } from "../../api/generated/CurrencyRate";
import type { RatesSettings } from "../../api/generated/RatesSettings";
import { hasCode } from "../../api/backend";
import { keys, useBackend } from "../../api/queries";
import { useLaunch } from "../../app/App";
import { DashButton } from "../../components/basics";
import { RATES_DISCLOSURE, ratesStatusText, useRateCurrencies, useRates } from "../../fiat/rates";
import { currencyName, fiatText, isLocalCurrency, localeCurrency } from "../../format/fiat";
import { failureText } from "../../onboarding/model";
import { Hint, Problem, SettingsCard } from "./parts";

export const LocalCurrencyText = {
  title: "Local currency",
  switchLabel: "Show values in local currency",
  emptyList: "The list of currencies fills after the first update.",
  noMatch: "No currency matches the search.",
  throttled: "Prices were just updated. Try again in a moment.",
};

export function LocalCurrencyCard() {
  const b = useBackend();
  const client = useQueryClient();
  const { network } = useLaunch();
  const rates = useRates();
  const view = rates.data;
  const on = !!view && view.settings.enabled && view.settings.disclosed;
  const currencies = useRateCurrencies(on);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const id = useId();
  if (!view) return null;

  const save = async (patch: Partial<RatesSettings>) => {
    setProblem(null);
    setBusy(true);
    try {
      await b.setRatesSettings({ ...view.settings, ...patch });
    } catch (e) {
      setProblem(failureText(e));
    } finally {
      setBusy(false);
      await client.invalidateQueries({ queryKey: keys.ratesAll });
    }
  };
  const refresh = async () => {
    setProblem(null);
    setBusy(true);
    try {
      await b.refreshRates();
    } catch (e) {
      setProblem(hasCode(e, "rates.throttled") ? LocalCurrencyText.throttled : failureText(e));
    } finally {
      setBusy(false);
      await client.invalidateQueries({ queryKey: keys.ratesAll });
    }
  };
  const status = ratesStatusText(view, b.locale);
  const mayRefresh = on && network.kind === "mainnet";
  return (
    <SettingsCard title={LocalCurrencyText.title} testId="settings-currency">
      <label className="checkbox-row">
        <input
          type="checkbox"
          data-testid="settings-currency-on"
          checked={on}
          disabled={busy}
          aria-describedby={`${id}-disclosure`}
          // On: the disclosure beside it was seen, and a first currency comes from the locale.
          onChange={(e) =>
            void save(e.target.checked ? { enabled: true, disclosed: true, currency: view.settings.currency ?? localeCurrency(b.locale) } : { enabled: false })
          }
        />
        {LocalCurrencyText.switchLabel}
      </label>
      <Hint id={`${id}-disclosure`}>{RATES_DISCLOSURE}</Hint>
      {on && <CurrencyPicker rows={currencies.data ?? []} chosen={view.settings.currency} busy={busy} onPick={(code) => void save({ currency: code })} />}
      {on && (
        <div className="settings-row settings-currency-status">
          <span className="field-hint" data-testid="settings-currency-status">
            {status}
          </span>
          {mayRefresh && (
            <DashButton variant="plainBlue" size="small" data-testid="settings-currency-refresh" disabled={busy} onClick={() => void refresh()}>
              Update now
            </DashButton>
          )}
        </div>
      )}
      <Problem text={problem} testId="settings-currency-problem" />
    </SettingsCard>
  );
}

function CurrencyPicker({ rows, chosen, busy, onPick }: { rows: CurrencyRate[]; chosen: string | null; busy: boolean; onPick: (code: string) => void }) {
  const b = useBackend();
  const [search, setSearch] = useState("");
  const id = useId();
  const local = rows.filter((r) => isLocalCurrency(r.code)).map((r) => ({ ...r, name: currencyName(r.code, b.locale) }));
  const q = search.trim().toLowerCase();
  const shown = q ? local.filter((r) => r.code.toLowerCase().includes(q) || r.name.toLowerCase().includes(q)) : local;
  return (
    <div className="currency-picker" data-testid="settings-currency-picker">
      {chosen && (
        <p className="settings-row-title" data-testid="settings-currency-chosen">
          {chosen}, {currencyName(chosen, b.locale)}
        </p>
      )}
      <div className="form-field">
        <label htmlFor={`${id}-search`}>Search currencies</label>
        <input
          id={`${id}-search`}
          type="search"
          data-testid="settings-currency-search"
          placeholder="Search by name or code"
          autoComplete="off"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
      </div>
      {local.length === 0 ? (
        <Hint>{LocalCurrencyText.emptyList}</Hint>
      ) : shown.length === 0 ? (
        <Hint>{LocalCurrencyText.noMatch}</Hint>
      ) : (
        <ul className="currency-list" aria-label="Currencies">
          {shown.map((r) => (
            <li key={r.code}>
              <button
                type="button"
                className="currency-row"
                data-testid={`settings-currency-${r.code}`}
                aria-pressed={r.code === chosen}
                disabled={busy}
                onClick={() => onPick(r.code)}
              >
                <span className="currency-code">{r.code}</span>
                <span className="currency-name">{r.name}</span>
                <span className="currency-price">{fiatText(100_000_000, r.price, r.code, b.locale)}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
