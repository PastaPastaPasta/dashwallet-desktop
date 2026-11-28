// Pay ▸ To a contact: a user the username search found who is not in the picker. Their card says
// what paying them takes, and offers it through Contacts' own actions: send a contact request,
// accept theirs and pay, or pay a contact the search reached first.

import { useEffect, useId, useRef, useState } from "react";
import type { EligibilityName } from "../../api/generated/EligibilityName";
import { Chip, DashButton } from "../../components/basics";
import type { ActionOutcome } from "../../contacts/actions";
import { unverifiedWarning } from "../../contacts/model";
import { useContactActions, useEligibility } from "../../contacts/queries";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { GrantSheet, Outcome } from "../tools/common";
import { useActionToast, useRefocus } from "../contacts/afterAction";
import { Avatar } from "../contacts/Avatar";
import { EnableButton, EnableFailure, SendRequestButton } from "../contacts/RequestButton";
import { foundLine, payeeOf, type Payee } from "./contact";
import type { FoundHit } from "./recipient";

export function FoundUser({
  walletId,
  identity,
  hit,
  onPay,
  onBack,
}: {
  walletId: string;
  identity: string;
  hit: FoundHit;
  onPay: (payee: Payee) => void;
  onBack: () => void;
}) {
  const t = useT();
  const actions = useModel(useContactActions(walletId, identity));
  const eligibility = useEligibility(walletId, identity, hit.identity);
  const toast = useActionToast(walletId);
  const payee = payeeOf({ contact: hit.identity, alias: null, displayName: hit.displayName?.trim() || null, username: hit.username });
  // A request sent here is pending at once (or, answering theirs, made them a contact), until an
  // eligibility read after it answers: they may accept meanwhile. A failed read's kept answer may
  // be stale: it shows Try again.
  const [requested, setRequested] = useState<{ answer: EligibilityName; at: number } | null>(null);
  const readSince = requested !== null && !eligibility.isError && eligibility.dataUpdatedAt > requested.at;
  const line = foundLine(payee.name, hit.relation, {
    data: requested && !readSince ? requested.answer : eligibility.isError ? undefined : eligibility.data,
    failed: !requested && eligibility.isError && !eligibility.isFetching,
  });
  // A request sent shows as the line; a refusal says why under it.
  const failed = actions.outcome?.contact === hit.identity && !actions.outcome.ok ? actions.outcome : null;

  // The card takes focus when it opens; once a request went, focus goes to the line saying so.
  const titleId = useId();
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => heading.current?.focus({ preventScroll: true }), []);
  const status = useRef<HTMLDivElement>(null);
  const arm = useRefocus(() => line?.next === "send", () => status.current);
  const sent = (o: ActionOutcome | null) => {
    toast(o, true);
    if (!o?.ok) return;
    setRequested({ answer: o.established ? "alreadyContact" : "pendingOutgoing", at: Date.now() });
    arm("send");
  };

  const warning = line?.next === "send" ? unverifiedWarning("send", hit.unverified) : line?.next === "acceptAndPay" ? unverifiedWarning("accept", hit.unverified) : null;
  let button = null;
  switch (line?.next) {
    case "send":
      button = <SendRequestButton key="send" actions={actions} contact={hit.identity} name={payee.name} size="small" testId="pay-found-send" onDone={sent} />;
      break;
    case "pay":
    case "acceptAndPay":
      button = (
        <DashButton key="pay" variant="filledBlue" size="small" data-testid="pay-found-pay" onClick={() => onPay(payee)}>
          {line.next === "pay" ? t("pay.contact.pay", { name: payee.name }) : t("pay.contact.acceptAndPay")}
        </DashButton>
      );
      break;
    case "enable":
      button = <EnableButton key="enable" actions={actions} size="small" testId="pay-found-enable" onDone={(o) => toast(o, true)} />;
      break;
    case "retry":
      button = (
        <DashButton key="retry" variant="plainBlue" size="small" onClick={() => void eligibility.refetch()}>
          {t("pay.contact.retry")}
        </DashButton>
      );
      break;
  }

  return (
    <div className="pay-form pay-found" data-testid="pay-found">
      <section className="pay-card card-l1 pay-payee-card" aria-labelledby={titleId} data-testid="pay-found-card">
        <span className="pay-payee">
          <Avatar letters={payee.letters} size="pane" identity={payee.contact} />
          <span className="pay-payee-names">
            <h2 className="pay-payee-name pay-found-name" id={titleId} tabIndex={-1} ref={heading} data-testid="pay-found-name">
              {payee.name}
            </h2>
            {payee.sub && <span className="contact-row-username">{payee.sub}</span>}
          </span>
          {hit.unverified && <Chip tone="warning">{t("pay.contact.unverified")}</Chip>}
        </span>
        <DashButton variant="tintedBlue" size="small" data-testid="pay-found-back" onClick={onBack}>
          {t("pay.contact.change")}
        </DashButton>
      </section>
      <div className="contacts-hint card-l1 pay-banner" role="status" ref={status} data-testid="pay-found-status">
        <span data-testid="pay-found-line">{line === undefined ? t("pay.contact.checking") : line.text}</span>
        {button}
      </div>
      {warning && (
        <p className="sheet-warning" role="note" data-testid="pay-found-unverified">
          {warning}
        </p>
      )}
      <Outcome ok={false} text={failed?.text ?? null} testId="pay-found-outcome" />
      <EnableFailure actions={actions} testId="pay-found-enable-outcome" />
      <GrantSheet grants={actions.grants} />
    </div>
  );
}
