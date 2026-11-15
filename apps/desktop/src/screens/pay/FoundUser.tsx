// Pay ▸ To a contact: a user the username search found who is not a contact yet. Their card says
// what paying them takes, and offers it through Contacts' own actions: send a contact request,
// accept theirs and pay, or pay a contact the search reached first.

import type { UserHit } from "../../api/generated/UserHit";
import { ui } from "../../app/ui";
import { Chip, DashButton } from "../../components/basics";
import { useContactActions, useEligibility } from "../../contacts/queries";
import { useModel } from "../../onboarding/model";
import { GrantSheet, Outcome } from "../tools/common";
import { useActionToast } from "../contacts/afterAction";
import { Avatar } from "../contacts/Avatar";
import { foundLine, payeeOf, type Payee } from "./contact";

export function FoundUser({
  walletId,
  identity,
  hit,
  onPay,
  onBack,
}: {
  walletId: string;
  identity: string;
  hit: UserHit;
  onPay: (payee: Payee) => void;
  onBack: () => void;
}) {
  const actions = useModel(useContactActions(walletId, identity));
  const eligibility = useEligibility(walletId, identity, hit.identity);
  const toast = useActionToast(walletId);
  const payee = payeeOf({ contact: hit.identity, alias: null, displayName: hit.displayName?.trim() || null, username: hit.username });
  const line = foundLine(payee.name, hit.relation, { data: eligibility.data, failed: eligibility.isError && !eligibility.isFetching });
  const outcome = actions.outcome?.contact === hit.identity ? actions.outcome : null;

  let button = null;
  switch (line?.next) {
    case "send":
      button = (
        <DashButton
          variant="filledBlue"
          size="small"
          data-testid="pay-found-send"
          disabled={actions.busy}
          onClick={() => void actions.send(hit.identity, payee.name, null).then((o) => toast(o, true))}
        >
          {actions.isRunning("send", hit.identity) ? "Sending…" : "Send Contact Request"}
        </DashButton>
      );
      break;
    case "pay":
    case "acceptAndPay":
      button = (
        <DashButton variant="filledBlue" size="small" data-testid="pay-found-pay" onClick={() => onPay(payee)}>
          {line.next === "pay" ? `Pay ${payee.name}` : "Accept and pay"}
        </DashButton>
      );
      break;
    case "enable":
      button = (
        <DashButton variant="tintedBlue" size="small" data-testid="pay-found-enable" disabled={actions.busy} onClick={() => void actions.enable().then((o) => toast(o, true))}>
          {actions.isRunning("enable", null) ? "Enabling…" : "Enable"}
        </DashButton>
      );
      break;
    case "retry":
      button = (
        <DashButton variant="plainBlue" size="small" onClick={() => void eligibility.refetch()}>
          Try again
        </DashButton>
      );
      break;
  }

  return (
    <div className="pay-form" data-testid="pay-found">
      <section className="pay-card card-l1 pay-payee-card" data-testid="pay-found-card">
        <span className="pay-payee">
          <Avatar letters={payee.letters} size="pane" identity={payee.contact} />
          <span className="pay-payee-names">
            <span className="pay-payee-name" data-testid="pay-found-name">
              {payee.name}
            </span>
            {payee.sub && <span className="contact-row-username">{payee.sub}</span>}
          </span>
          {hit.unverified && <Chip tone="warning">Unverified</Chip>}
        </span>
        <DashButton variant="tintedBlue" size="small" data-testid="pay-found-back" onClick={onBack}>
          Change
        </DashButton>
      </section>
      <div className="contacts-hint card-l1 pay-banner" role="status" data-testid="pay-found-status">
        <span data-testid="pay-found-line">{line === undefined ? "Checking…" : line.text}</span>
        {button}
      </div>
      <Outcome ok={outcome?.ok ?? true} text={outcome?.text ?? null} testId="pay-found-outcome" />
      <p className="placeholder-note">
        <DashButton variant="plainBlue" size="small" data-testid="pay-contact-to-address" onClick={() => ui.set({ payTab: "address" })}>
          Pay to an address instead
        </DashButton>
      </p>
      <GrantSheet grants={actions.grants} />
    </div>
  );
}
