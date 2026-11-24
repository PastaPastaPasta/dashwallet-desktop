// Pay ▸ To an address's added recipients (QT-052, dash-qt's Add Recipient): each row is an address,
// an amount and a label, checked as the first recipient's are, and removable. One recipient at most
// pays the fee from its amount (`receivedAmounts`), so the form turns the others off.

import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { Recipient } from "../../api/generated/Recipient";
import { useBackend } from "../../api/queries";
import { useDebounced } from "../../app/useDebounced";
import { DashButton } from "../../components/basics";
import { FiatSecondary, FiatToggle } from "../../fiat/entry";
import type { useDisplay } from "../../format/display";
import { useT } from "../../i18n";
import { FormField, SETTLE_MS, usePayAmount } from "./form";
import { addressProblem, recipientOf } from "./model";

type Shown = ReturnType<typeof useDisplay>;

export interface MoreRow {
  id: number;
  /** The row's recipient once complete and checked; `null` while it is not. */
  recipient: Recipient | null;
  /** Its amount as parsed, whatever the address: Max pays around it. */
  amount: number | null;
  /** Its amount field settled (Max waits for every row). */
  settled: boolean;
  subtractFee: boolean;
}

export function MoreRecipient({
  row,
  number,
  network,
  shown,
  shieldedRoute,
  fieldError,
  onChange,
  onSubtractFee,
  onRemove,
}: {
  row: MoreRow;
  /** Its place in the payment, 2 for the first added one: the label, and the engine's index + 1. */
  number: number;
  network: NetworkInfo;
  shown: Shown;
  /** The wallet can send from Shielded: a Shielded address is pointed there. */
  shieldedRoute: boolean;
  fieldError: (f: "address" | "amount") => string | null;
  onChange: (recipient: Recipient | null, amount: number | null, settled: boolean) => void;
  onSubtractFee: (on: boolean) => void;
  onRemove: () => void;
}) {
  const t = useT();
  const b = useBackend();
  const [address, setAddress] = useState("");
  const [label, setLabel] = useState("");
  const amount = usePayAmount(shown, "");
  const addr = address.trim();
  const checked = useDebounced(addr, SETTLE_MS);
  const cls = useQuery({
    queryKey: ["classifyAddress", network.dirName, checked],
    queryFn: () => b.classifyAddress(network.dirName, checked),
    enabled: checked !== "",
    staleTime: Infinity,
  });
  // A Platform address is not paid in a Core transaction: only the first recipient can be one.
  const addrError =
    checked !== "" && cls.data
      ? cls.data.kind === "platform"
        ? t("pay.more.platform")
        : cls.data.kind === "shielded" && shieldedRoute
          ? t("pay.more.shielded")
          : addressProblem(cls.data)
      : null;
  const { duffs } = amount;
  const recipient =
    checked === addr && amount.settled && !amount.fiat.error && cls.data?.kind === "core" && typeof duffs === "number" && duffs > 0
      ? recipientOf({ address: checked, amount: duffs, subtractFee: row.subtractFee, label, message: "" })
      : null;
  const parsed = typeof duffs === "number" && duffs > 0 ? duffs : null;
  const key = JSON.stringify([recipient, parsed, amount.settled]);
  useEffect(() => {
    onChange(recipient, parsed, amount.settled);
    // `key` is the recipient's value: a new object of the same value is no change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  const { fiat } = amount;
  return (
    <section className="pay-card card-l1 pay-more" data-testid={`pay-more-${number}`} aria-label={t("pay.more.recipient", { number })}>
      <div className="pay-more-head">
        <h3 className="pay-more-title">{t("pay.more.recipient", { number })}</h3>
        <DashButton variant="plainBlue" size="small" data-testid={`pay-more-${number}-remove`} onClick={onRemove}>
          {t("pay.more.remove")}
        </DashButton>
      </div>
      <FormField label={t("pay.toAddress.payTo")} error={addrError ?? fieldError("address")}>
        {(p) => (
          <input
            {...p}
            data-testid={`pay-more-${number}-address`}
            data-autofocus
            value={address}
            autoComplete="off"
            spellCheck={false}
            placeholder={t("pay.toAddress.placeholder")}
            onChange={(e) => setAddress(e.target.value)}
          />
        )}
      </FormField>
      <FormField
        label={t("pay.form.amount", { unit: fiat.unitName })}
        error={fiat.error ?? amount.error ?? fieldError("amount")}
        hint={fiat.toggle.shown && <FiatSecondary entry={fiat} duffs={amount.duffs} testId={`pay-more-${number}-amount-other`} />}
      >
        {(p) => (
          <span className="amount-entry">
            <input
              {...p}
              data-testid={`pay-more-${number}-amount`}
              value={fiat.value}
              inputMode="decimal"
              autoComplete="off"
              placeholder={fiat.placeholder}
              onChange={(e) => fiat.onChange(e.target.value)}
            />
            <FiatToggle entry={fiat} shown={shown} testId={`pay-more-${number}-amount-toggle`} />
          </span>
        )}
      </FormField>
      <label className="checkbox-row">
        <input type="checkbox" data-testid={`pay-more-${number}-subtract-fee`} checked={row.subtractFee} onChange={(e) => onSubtractFee(e.target.checked)} />
        {t("pay.form.subtractFee")}
      </label>
      <FormField label={t("pay.toAddress.label")} hint={t("pay.toAddress.labelHint")}>
        {(p) => <input {...p} data-testid={`pay-more-${number}-label`} value={label} autoComplete="off" onChange={(e) => setLabel(e.target.value)} />}
      </FormField>
    </section>
  );
}
