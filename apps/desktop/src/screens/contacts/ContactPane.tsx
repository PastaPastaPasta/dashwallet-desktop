// The Contacts detail slot (DP2-07 D11, D12; DP2-09): a `dashpay://user` link's card (U-01), else
// Add contact's placeholder (DP2-08), else the selected contact's pane (its actions, payment
// activity and private details), else a prompt to pick one.

import { useId, useState } from "react";
import type { ActivityFilterName } from "../../api/generated/ActivityFilterName";
import type { ContactDetailInfo } from "../../api/generated/ContactDetailInfo";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { errorMessage } from "../../api/backend";
import { AmountText, DashButton, Icon } from "../../components/basics";
import { CopyValue } from "../../components/detail";
import { useRowContext } from "../../components/history";
import { navigate, ui, type AddContactIntent, type ContactPrefill } from "../../app/ui";
import type { ContactActions } from "../../contacts/actions";
import { avatarLetters, channelTitle, relationTitle, rowChips, shownName, subtitle } from "../../contacts/model";
import { ACTIVITY_FILTERS, activityRow, detailsChanged, detailsForm, detailsOf, paneActions, type DetailsForm } from "../../contacts/pane";
import { useContactActivity, useContactDetail } from "../../contacts/queries";
import { useModel } from "../../onboarding/model";
import { detailDate } from "../../format/tx";
import { Outcome } from "../tools/common";
import { RowChips } from "./ContactList";
import { Avatar } from "./Avatar";

/** What the selected contact's pane acts with (DP2-09 D16): the identity's actions, and whether
 * it has its DashPay keys (`undefined` while unknown). */
export interface PaneContext {
  walletId: string;
  identity: string;
  network: NetworkInfo;
  actions: ContactActions;
  hasKeys: boolean | undefined;
}

export function ContactPane({
  prefill,
  add,
  selected,
  pane,
}: {
  prefill: ContactPrefill | null;
  add: AddContactIntent | null;
  selected: ContactSummaryInfo | null;
  pane: PaneContext | null;
}) {
  let slot;
  if (prefill) slot = <LinkCard prefill={prefill} />;
  else if (add) slot = <AddContact add={add} />;
  else if (selected && pane) slot = <Selected key={selected.contact} contact={selected} pane={pane} />;
  else
    slot = (
      <p className="placeholder card-l1" data-testid="contact-pane-empty">
        Select a contact to see the details.
      </p>
    );
  return (
    <aside className="contacts-pane" aria-label="Contact" data-testid="contacts-pane">
      {slot}
    </aside>
  );
}

/** U-01's card for a `dashpay://user` link (its D12): the link's username and identity, unverified. */
function LinkCard({ prefill }: { prefill: ContactPrefill }) {
  return (
    <section className="add-contact card-l1" aria-labelledby="add-contact-title" data-testid="add-contact">
      <h2 id="add-contact-title" className="add-contact-title">
        Add contact
      </h2>
      <p data-testid="add-contact-username">@{prefill.username}</p>
      <p className="mono" data-testid="add-contact-identity">
        {prefill.identity}
      </p>
      <p className="placeholder-note">The username comes from the link and is not verified yet.</p>
      <span className="add-contact-actions">
        <DashButton variant="filledWhiteBlue" disabled>
          Send contact request
        </DashButton>
        <DashButton variant="plainBlue" onClick={() => ui.set({ contactPrefill: null })}>
          Dismiss
        </DashButton>
      </span>
    </section>
  );
}

/** Add contact's slot (D12), which DP2-08 fills: what opened it, and a "not yet". */
function AddContact({ add }: { add: AddContactIntent }) {
  return (
    <section className="add-contact card-l1" aria-labelledby="add-contact-slot-title" data-testid="placeholder-add-contact">
      <h2 id="add-contact-slot-title" className="add-contact-title">
        Add contact
      </h2>
      {add.hit ? (
        <>
          <p data-testid="add-contact-hit-username">@{add.hit.username}</p>
          <p className="mono">{add.hit.identity}</p>
        </>
      ) : (
        add.query && <p data-testid="add-contact-query">Usernames starting with “{add.query}”</p>
      )}
      <p className="placeholder-note">Searching for users and sending contact requests are not available yet.</p>
      <span className="add-contact-actions">
        <DashButton variant="plainBlue" onClick={() => ui.set({ addContact: null })}>
          Close
        </DashButton>
      </span>
    </section>
  );
}

/** When the row's time was: a contact's start, else its request's. */
const SINCE: Record<ContactSummaryInfo["relation"], string> = {
  contact: "Contact since",
  pendingOutgoing: "Request sent",
  pendingIncoming: "Request received",
  ignored: "Request received",
  stranger: "Since",
  isSelf: "Since",
};

/** The selected contact: its row's facts at once, then what its detail adds (DP2-09 D1). */
function Selected({ contact: c, pane }: { contact: ContactSummaryInfo; pane: PaneContext }) {
  const sub = subtitle(c);
  const chips = rowChips(c);
  const detail = useContactDetail(pane.walletId, pane.identity, c.contact);
  return (
    <section className="add-contact contact-detail card-l1" aria-labelledby="contact-detail-name" data-testid="contact-detail">
      <header className="contact-detail-header">
        <Avatar letters={avatarLetters(c)} size="pane" />
        <div className="contact-detail-names">
          <h2 id="contact-detail-name" className="add-contact-title">
            {shownName(c)}
          </h2>
          {sub && <p className="contact-row-username">{sub}</p>}
          {chips.length > 0 && (
            <p className="contact-detail-chips">
              <RowChips chips={chips} />
            </p>
          )}
        </div>
      </header>
      <dl className="about-list contact-detail-facts">
        <dt>Identity</dt>
        <dd data-testid="contact-detail-identity">
          <CopyValue value={c.contact} label="Copy identity" mono />
        </dd>
        <dt>Relation</dt>
        <dd data-testid="contact-detail-relation">{relationTitle(c.relation)}</dd>
        {c.relation === "contact" && (
          <>
            <dt>Payments</dt>
            <dd data-testid="contact-detail-channel">{channelTitle(c.channel)}</dd>
          </>
        )}
        {c.since !== null && (
          <>
            <dt>{SINCE[c.relation]}</dt>
            <dd>{detailDate(new Date(c.since * 1000))}</dd>
          </>
        )}
      </dl>
      <PaneButtons contact={c} pane={pane} />
      {c.relation === "contact" && (
        <>
          {detail.data && <PrivateDetails key={JSON.stringify(detail.data.privateDetails)} contact={c} detail={detail.data} actions={pane.actions} />}
          {detail.isError && (
            <p className="form-problem" role="alert">
              Could not read this contact: {errorMessage(detail.error)}
            </p>
          )}
          <Activity contact={c.contact} pane={pane} />
        </>
      )}
    </section>
  );
}

/** The relation's actions (D2) and the last outcome for this contact (D4). */
function PaneButtons({ contact: c, pane }: { contact: ContactSummaryInfo; pane: PaneContext }) {
  const actions = useModel(pane.actions);
  const offer = paneActions(c, pane.hasKeys);
  const busy = actions.running !== null;
  const outcome = actions.outcome?.contact === c.contact && actions.outcome.kind !== "details" ? actions.outcome : null;
  let buttons;
  let note: string | null = null;
  switch (offer.kind) {
    case "request":
      note = offer.acceptBlocked;
      buttons = (
        <>
          <DashButton
            variant="filledBlue"
            data-testid="contact-accept"
            disabled={busy || offer.acceptBlocked !== null}
            onClick={() => void actions.accept(c.contact, shownName(c))}
          >
            {actions.isRunning("accept", c.contact) ? "Accepting…" : "Accept"}
          </DashButton>
          <DashButton variant="tintedGray" data-testid="contact-ignore" disabled={busy} onClick={() => void actions.ignore(c.contact)}>
            {actions.isRunning("ignore", c.contact) ? "Ignoring…" : "Ignore"}
          </DashButton>
        </>
      );
      break;
    case "sent":
      note = "Contact request sent. You become contacts once they accept it.";
      break;
    case "contact":
      note = offer.payBlocked;
      buttons = (
        <DashButton variant="filledBlue" data-testid="contact-pay" disabled={offer.payBlocked !== null} onClick={() => navigate("pay", { payTab: "contact" })}>
          Pay
        </DashButton>
      );
      break;
    case "ignored":
      note = "You ignored this contact request.";
      buttons = (
        <DashButton variant="tintedBlue" data-testid="contact-unignore" disabled={busy} onClick={() => void actions.unignore(c.contact)}>
          {actions.isRunning("unignore", c.contact) ? "Unignoring…" : "Unignore"}
        </DashButton>
      );
      break;
  }
  return (
    <div className="contact-pane-actions" data-testid="contact-pane-actions">
      {buttons && <span className="add-contact-actions">{buttons}</span>}
      {note && (
        <p className="placeholder-note" data-testid="contact-pane-note">
          {note}
        </p>
      )}
      <Outcome ok={outcome?.ok ?? true} text={outcome?.text ?? null} testId="contact-pane-outcome" />
    </div>
  );
}

/** "Only visible to you" (D7): alias, note and Hide, saved and published together. */
function PrivateDetails({ contact: c, detail, actions: model }: { contact: ContactSummaryInfo; detail: ContactDetailInfo; actions: ContactActions }) {
  const actions = useModel(model);
  const id = useId();
  const saved = detail.privateDetails;
  const [form, setForm] = useState<DetailsForm>(() => detailsForm(saved));
  const busy = actions.running !== null;
  const saving = actions.isRunning("details", c.contact);
  const outcome = actions.outcome?.contact === c.contact && actions.outcome.kind === "details" ? actions.outcome : null;
  const save = (f: DetailsForm) => void actions.saveDetails(c.contact, detailsOf(f));
  return (
    <section className="contact-pane-section" aria-labelledby={`${id}-title`} data-testid="contact-private">
      <h3 id={`${id}-title`} className="contact-pane-heading">
        Only visible to you
      </h3>
      <form
        className="contact-details-form"
        onSubmit={(e) => {
          e.preventDefault();
          save(form);
        }}
      >
        <label className="form-field">
          Alias
          <input data-testid="contact-alias" value={form.alias} disabled={busy} onChange={(e) => setForm({ ...form, alias: e.target.value })} />
        </label>
        <label className="form-field">
          Note
          <textarea className="contact-note" data-testid="contact-note" rows={3} value={form.note} disabled={busy} onChange={(e) => setForm({ ...form, note: e.target.value })} />
        </label>
        <label className="check-row contact-hide">
          <input
            type="checkbox"
            data-testid="contact-hide"
            checked={form.hidden}
            disabled={busy}
            onChange={(e) => {
              // Hide saves at once, with the alias and note as last saved (D7).
              const next = { ...detailsForm(saved), hidden: e.target.checked };
              setForm({ ...form, hidden: next.hidden });
              save(next);
            }}
          />
          Hide this contact
        </label>
        <span className="add-contact-actions">
          <DashButton variant="tintedBlue" type="submit" data-testid="contact-details-save" disabled={busy || !detailsChanged(form, saved)}>
            {saving ? "Saving…" : "Save"}
          </DashButton>
        </span>
        <Outcome ok={outcome?.ok ?? true} text={outcome?.text ?? null} testId="contact-details-outcome" />
      </form>
    </section>
  );
}

/** The payments with the contact (D8): All / Sent / Received, newest first, a page at a time. */
function Activity({ contact, pane }: { contact: string; pane: PaneContext }) {
  const id = useId();
  const [filter, setFilter] = useState<ActivityFilterName>("all");
  const activity = useContactActivity(pane.walletId, pane.identity, contact, filter, true);
  const ctx = useRowContext(pane.network);
  const items = activity.data?.pages.flatMap((p) => p.items) ?? [];
  return (
    <section className="contact-pane-section" aria-labelledby={`${id}-title`} data-testid="contact-activity">
      <h3 id={`${id}-title`} className="contact-pane-heading">
        Activity
      </h3>
      <div className="segmented" role="group" aria-label="Show payments" data-testid="contact-activity-filter">
        {ACTIVITY_FILTERS.map((f) => (
          <button key={f.id} type="button" className="segment" aria-pressed={filter === f.id} onClick={() => setFilter(f.id)}>
            {f.title}
          </button>
        ))}
      </div>
      {activity.isPending && <p className="placeholder-note">Loading payments…</p>}
      {activity.isError && (
        <p className="form-problem" role="alert">
          Could not load the payments: {errorMessage(activity.error)}{" "}
          <DashButton variant="plainBlue" size="small" onClick={() => void activity.refetch()}>
            Try again
          </DashButton>
        </p>
      )}
      {activity.isSuccess && items.length === 0 && (
        <p className="placeholder-note" data-testid="contact-activity-empty">
          No payments with this contact yet.
        </p>
      )}
      {items.length > 0 && (
        <ul className="tx-group-rows contact-activity-rows">
          {items.map((item) => {
            const row = activityRow(item);
            const when = item.timestamp === null ? null : new Date(item.timestamp * 1000);
            return (
              <li key={`${item.txid}:${item.direction}`} className="tx-row-item">
                <button type="button" className="tx-row" data-testid="contact-activity-row" data-txid={item.txid} onClick={() => ui.set({ detail: item.txid })}>
                  <Icon id={item.direction === "sent" ? "txSent" : "txReceived"} size={30} className="tx-row-icon" />
                  <span className="tx-row-text">
                    <span className="tx-row-title">{row.title}</span>
                    <span className="tx-row-subtitle">{row.pending ? "Pending" : when ? detailDate(when, ctx.locale) : ""}</span>
                  </span>
                  <span className="tx-row-trailing">
                    <AmountText className="tx-row-amount" text={ctx.display.compact(row.signed, true)} unitName={ctx.display.name} />
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
      {activity.hasNextPage && (
        <DashButton
          variant="plainBlue"
          size="small"
          data-testid="contact-activity-more"
          disabled={activity.isFetchingNextPage}
          onClick={() => void activity.fetchNextPage()}
        >
          {activity.isFetchingNextPage ? "Loading…" : "Show more"}
        </DashButton>
      )}
    </section>
  );
}
