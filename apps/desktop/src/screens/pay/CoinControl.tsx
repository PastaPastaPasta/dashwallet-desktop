// Advanced: coin control in Pay (U-03 D7, QT-067…075). Which coins pay (any, fully mixed only,
// or a selection with dash-qt's summary), the fee (a confirmation target or a custom rate) and
// where change goes. Collapsed by default; nothing changes until the user picks something.

import { useEffect, useId, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { errorMessage } from "../../api/backend";
import type { FeePolicy } from "../../api/generated/FeePolicy";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { Utxo } from "../../api/generated/Utxo";
import { useBackend, useFeePolicy, useUtxos } from "../../api/queries";
import { useDebounced } from "../../app/useDebounced";
import { AmountText } from "../../components/basics";
import { LoadingState } from "../../components/history";
import { compact, full } from "../../format/amount";
import { shortAddress } from "../../format/tx";
import { DEFAULT_TARGET_BLOCKS, feeMode, outpointKey, outpointOf, type Advanced, type CoinChoice } from "./model";

/** Dash's target block interval: 2.5 minutes. */
const BLOCK_MINUTES = 2.5;

const COIN_CHOICES: { id: CoinChoice; title: string }[] = [
  { id: "any", title: "Any available funds" },
  { id: "mixed", title: "Fully mixed funds only (CoinJoin)" },
  { id: "selected", title: "Selected coins" },
];

/** Why a coin cannot be chosen, or `null` when it can. */
function notSelectable(u: Utxo): string | null {
  if (u.spendable) return null;
  if (u.reserved) return "Reserved by a payment in progress";
  if (u.userLocked) return "Locked";
  if (u.isCoinbase) return "Immature";
  return "Not spendable";
}

function targetTitle(blocks: number): string {
  const minutes = blocks * BLOCK_MINUTES;
  const time = minutes < 60 ? `${minutes} minutes` : `${+(minutes / 60).toFixed(1)} hours`;
  return `${time} (${blocks} ${blocks === 1 ? "block" : "blocks"})`;
}

/** Why a typed custom rate cannot be used, or `null` when it can. */
function rateProblem(rate: number | null | undefined, policy: FeePolicy, unit: string): string | null {
  if (rate === null || rate === undefined) return "Enter a fee rate in DASH per kB.";
  if (rate < policy.minRelayPerKb) return `The fee rate must be at least ${full(policy.minRelayPerKb, false)} ${unit}/kB (the minimum relay fee).`;
  if (rate > policy.maxCustomPerKb) return `The fee rate must be at most ${full(policy.maxCustomPerKb, false)} ${unit}/kB.`;
  return null;
}

function Coins({ walletId, network, advanced, onChange, payAmount }: Props) {
  const b = useBackend();
  const utxos = useUtxos(walletId, true);
  const selected = new Set(advanced.selected);
  const fee = feeMode(advanced);
  const summary = useQuery({
    queryKey: ["coins", walletId, "summary", advanced.selected, payAmount, fee] as const,
    queryFn: () => b.coinSelectionSummary(walletId, advanced.selected.map(outpointOf), payAmount ? [payAmount] : [], fee!),
    enabled: advanced.selected.length > 0 && !!fee,
  });
  const toggle = (key: string, on: boolean) =>
    onChange({ ...advanced, selected: on ? [...advanced.selected, key] : advanced.selected.filter((k) => k !== key) });
  const unit = network.unitName;
  if (utxos.isPending) return <LoadingState testId="coins-loading" text="Loading coins" />;
  if (utxos.isError) return <p className="form-problem">Could not load the coins: {errorMessage(utxos.error)}</p>;
  if (utxos.data.length === 0) return <p className="placeholder-note">This wallet has no coins yet.</p>;
  const s = summary.data;
  return (
    <>
      <table className="data-table coin-table" data-testid="coin-table">
        <caption className="sr-only">Coins of this wallet</caption>
        <thead>
          <tr>
            <th scope="col">
              <span className="sr-only">Select</span>
            </th>
            <th scope="col" className="num">
              Amount ({unit})
            </th>
            <th scope="col">Address / Label</th>
            <th scope="col" className="num">
              Confirmations
            </th>
            <th scope="col">Status</th>
          </tr>
        </thead>
        <tbody>
          {utxos.data.map((u) => {
            const key = outpointKey(u.outpoint);
            const why = notSelectable(u);
            const party = u.label || shortAddress(u.address);
            return (
              <tr key={key} data-testid={`coin-${u.outpoint.txid.slice(0, 8)}-${u.outpoint.vout}`}>
                <td>
                  <input
                    type="checkbox"
                    aria-label={`Use ${compact(u.amount, false)} ${unit} at ${party}`}
                    checked={selected.has(key)}
                    disabled={why !== null && !selected.has(key)}
                    onChange={(e) => toggle(key, e.target.checked)}
                  />
                </td>
                <td className="num">{compact(u.amount, false)}</td>
                <td title={u.address}>{party}</td>
                <td className="num">{u.confirmations}</td>
                <td>{why ?? (u.coinjoinRounds !== null ? `Mixed, ${u.coinjoinRounds} rounds` : u.isChange ? "Change" : "")}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {s && (
        <dl className="coin-summary" data-testid="coin-summary" aria-live="polite">
          <div>
            <dt>Quantity</dt>
            <dd>{s.quantity}</dd>
          </div>
          <div>
            <dt>Amount</dt>
            <dd>
              <AmountText text={full(s.amount, false)} unitName={unit} glyph={false} />
            </dd>
          </div>
          <div>
            <dt>Fee</dt>
            <dd>
              ≈ <AmountText text={full(s.fee, false)} unitName={unit} glyph={false} />
            </dd>
          </div>
          <div>
            <dt>After fee</dt>
            <dd>
              ≈ <AmountText text={full(s.afterFee, false)} unitName={unit} glyph={false} />
            </dd>
          </div>
          <div>
            <dt>Bytes</dt>
            <dd>≈ {s.bytes}</dd>
          </div>
          <div>
            <dt>Change</dt>
            <dd>
              ≈ <AmountText text={full(s.change, false)} unitName={unit} glyph={false} />
            </dd>
          </div>
        </dl>
      )}
      {s?.insufficientFunds && (
        <p className="form-problem" data-testid="coin-insufficient">
          Insufficient funds! The selected coins do not cover the amount and the fee.
        </p>
      )}
    </>
  );
}

function Fee({ advanced, onChange, network }: Props) {
  const b = useBackend();
  const policy = useFeePolicy();
  const [text, setText] = useState("");
  const custom = useDebounced(text.trim(), 300);
  const id = useId();
  const parsed = useQuery({ queryKey: ["parseAmount", custom], queryFn: () => b.parseAmount(custom), enabled: custom !== "", staleTime: Infinity });
  const p = policy.data;
  const rate = parsed.data;
  const problem = advanced.fee.kind !== "custom" || custom === "" || parsed.isPending || !p ? null : rateProblem(rate, p, network.unitName);
  // Still typing (`text` ahead of the debounced `custom`) or parsing: the old rate must not be
  // the one Review confirms.
  const settled = custom === text.trim() && !parsed.isPending;
  const good = settled && rate !== null && rate !== undefined && problem === null && custom !== "" ? rate : null;
  // The parsed rate goes up once it settles; `null` holds the payment until it is valid.
  useEffect(() => {
    if (advanced.fee.kind === "custom" && advanced.fee.duffsPerKb !== good) onChange({ ...advanced, fee: { kind: "custom", duffsPerKb: good } });
  }, [advanced, good, onChange]);
  const targets = p?.targets ?? [{ targetBlocks: DEFAULT_TARGET_BLOCKS, duffsPerKb: 0 }];
  return (
    <fieldset className="radio-group" data-testid="fee-choice">
      <legend>Transaction fee</legend>
      <label className="radio-row">
        <input
          type="radio"
          name={`${id}-fee`}
          checked={advanced.fee.kind === "recommended"}
          onChange={() => onChange({ ...advanced, fee: { kind: "recommended", targetBlocks: DEFAULT_TARGET_BLOCKS } })}
        />
        Recommended
      </label>
      {advanced.fee.kind === "recommended" && (
        <label className="inline-field">
          Confirmation time target
          <select
            data-testid="fee-target"
            value={advanced.fee.targetBlocks}
            onChange={(e) => onChange({ ...advanced, fee: { kind: "recommended", targetBlocks: Number(e.target.value) } })}
          >
            {targets.map((t) => (
              <option key={t.targetBlocks} value={t.targetBlocks}>
                {targetTitle(t.targetBlocks)}
              </option>
            ))}
          </select>
        </label>
      )}
      <label className="radio-row">
        <input
          type="radio"
          name={`${id}-fee`}
          data-testid="fee-custom"
          checked={advanced.fee.kind === "custom"}
          onChange={() => onChange({ ...advanced, fee: { kind: "custom", duffsPerKb: good } })}
        />
        Custom
      </label>
      {advanced.fee.kind === "custom" && (
        <span className="inline-field-column">
          <label className="inline-field" htmlFor={`${id}-rate`}>
            Fee rate ({network.unitName}/kB)
          </label>
          <input
            id={`${id}-rate`}
            data-testid="fee-rate"
            value={text}
            inputMode="decimal"
            placeholder={p ? full(p.minRelayPerKb, false) : ""}
            aria-invalid={!!problem}
            aria-describedby={problem ? `${id}-rate-error` : undefined}
            onChange={(e) => setText(e.target.value)}
          />
          {problem && (
            <span id={`${id}-rate-error`} className="field-error">
              {problem}
            </span>
          )}
        </span>
      )}
    </fieldset>
  );
}

function Change({ advanced, onChange }: Props) {
  const id = useId();
  const custom = advanced.changeAddress !== null;
  return (
    <div className="change-choice">
      <label className="checkbox-row">
        <input
          type="checkbox"
          data-testid="change-custom"
          checked={custom}
          onChange={(e) => onChange({ ...advanced, changeAddress: e.target.checked ? "" : null })}
        />
        Custom change address
      </label>
      {custom && (
        <span className="inline-field-column">
          <label className="sr-only" htmlFor={id}>
            Change address
          </label>
          <input
            id={id}
            data-testid="change-address"
            value={advanced.changeAddress ?? ""}
            placeholder="Dash address for the change"
            spellCheck={false}
            onChange={(e) => onChange({ ...advanced, changeAddress: e.target.value })}
          />
        </span>
      )}
    </div>
  );
}

interface Props {
  walletId: string;
  network: NetworkInfo;
  advanced: Advanced;
  onChange: (a: Advanced) => void;
  /** The amount being paid, duffs, when it parses: coin control's summary pays it. */
  payAmount: number | null;
}

export function CoinControl(props: Props) {
  const { advanced, onChange } = props;
  const id = useId();
  return (
    <details className="disclosure card-l1 coin-control" data-testid="pay-advanced">
      <summary>Advanced: coins, fee and change</summary>
      <div className="coin-control-body">
        <fieldset className="radio-group" data-testid="coin-choice">
          <legend>Pay with</legend>
          {COIN_CHOICES.map((c) => (
            <label key={c.id} className="radio-row">
              <input
                type="radio"
                name={`${id}-coins`}
                data-testid={`coin-choice-${c.id}`}
                checked={advanced.coins === c.id}
                onChange={() => onChange({ ...advanced, coins: c.id })}
              />
              {c.title}
            </label>
          ))}
        </fieldset>
        {advanced.coins === "selected" && <Coins {...props} />}
        <Fee {...props} />
        <Change {...props} />
      </div>
    </details>
  );
}
