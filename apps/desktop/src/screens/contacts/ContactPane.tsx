// The Contacts detail slot (DP2-07 D11, D12; DP2-09): Add Contact (DP2-08, a search, a scan or a
// `dashpay://user` link), else the selected contact's pane (its actions, payment activity and
// private details), else a prompt to pick one.

import { useId, useRef, useState, type ReactNode } from "react";
import type { ActivityFilterName } from "../../api/generated/ActivityFilterName";
import type { ContactDetailInfo } from "../../api/generated/ContactDetailInfo";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { errorMessage } from "../../api/backend";
import { AmountText, DashButton, Icon } from "../../components/basics";
import { CopyValue } from "../../components/detail";
import { useRowContext } from "../../components/history";
import { navigate, ui } from "../../app/ui";
import type { ActionOutcome, ContactActions } from "../../contacts/actions";
import { avatarLetters, channelTitle, relationTitle, rowChips, shownName, subtitle } from "../../contacts/model";
import { ACTIVITY_FILTERS, activityRow, detailsChanged, detailsForm, detailsOf, paneActions, type DetailsForm } from "../../contacts/pane";
import { useContactActivity, useContactDetail } from "../../contacts/queries";
import { useModel } from "../../onboarding/model";
import { detailDate } from "../../format/tx";
import { Outcome } from "../tools/common";
import { RowChips } from "./ContactList";
import { useActionToast, useRefocus } from "./afterAction";
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

export function ContactPane({ adding, selected, pane }: { adding: ReactNode; selected: ContactSummaryInfo | null; pane: PaneContext | null }) {
  let slot;
  if (adding) slot = adding;
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
  // Only a contact has private details (DP2-09 D7).
  const detail = useContactDetail(pane.walletId, pane.identity, c.relation === "contact" ? c.contact : null);
  return (
    <section className="add-contact contact-detail card-l1" aria-labelledby="contact-detail-name" data-testid="contact-detail">
      <header className="contact-detail-header">
        <Avatar letters={avatarLetters(c)} size="pane" identity={c.contact} />
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
      <PaneButtons contact={c} pane={pane} locked={!!detail.data?.paymentLock} />
      {c.relation === "contact" && (
        <>
          {detail.data && <PrivateDetails walletId={pane.walletId} contact={c} detail={detail.data} actions={pane.actions} />}
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

/** The relation's actions (D2) and the last outcome for this contact (D4). Pay and "Accept and
 * pay" open Pay ▸ To a contact on it (DP3-06 D15); `locked`: a payment to it may have been sent. */
function PaneButtons({ contact: c, pane, locked }: { contact: ContactSummaryInfo; pane: PaneContext; locked: boolean }) {
  const actions = useModel(pane.actions);
  const offer = paneActions(c, pane.hasKeys);
  const busy = actions.busy;
  const outcome = actions.outcome?.contact === c.contact && actions.outcome.kind !== "details" ? actions.outcome : null;
  // Accept, Ignore and Unignore change the relation, and their buttons go with it: focus goes to
  // what the pane offers now and the outcome (D28); a toast says an outcome the pane cannot.
  const toast = useActionToast(pane.walletId);
  const area = useRef<HTMLDivElement>(null);
  const arm = useRefocus((kind) => kind === offer.kind, () => area.current);
  const done = (o: ActionOutcome | null) => {
    toast(o, true);
    if (o?.ok) arm(offer.kind);
  };
  const pay = () => navigate("pay", { payTab: "contact", payContact: { walletId: pane.walletId, contact: c.contact } });
  let buttons;
  let note: string | null = null;
  switch (offer.kind) {
    case "request":
      note = offer.acceptBlocked;
      buttons = (
        <>
          <DashButton
            key="accept"
            variant="filledBlue"
            data-testid="contact-accept"
            disabled={busy || offer.acceptBlocked !== null}
            onClick={() => void actions.accept(c.contact, shownName(c)).then(done)}
          >
            {actions.isRunning("accept", c.contact) ? "Accepting…" : "Accept"}
          </DashButton>
          <DashButton key="acceptPay" variant="tintedBlue" data-testid="contact-accept-pay" disabled={busy || offer.acceptBlocked !== null} onClick={pay}>
            Accept and pay
          </DashButton>
          <DashButton key="ignore" variant="tintedGray" data-testid="contact-ignore" disabled={busy} onClick={() => void actions.ignore(c.contact).then(done)}>
            {actions.isRunning("ignore", c.contact) ? "Ignoring…" : "Ignore"}
          </DashButton>
        </>
      );
      break;
    case "sent":
      note = "Contact request sent. You become contacts once they accept it.";
      break;
    case "contact":
      // A lock resolves on the Pay tab, so Pay leads there even on a broken channel.
      note = locked ? "A payment to this contact may have been sent. Pay shows it and checks it again." : (offer.payBlocked ?? offer.payNote);
      buttons = (
        <DashButton key="pay" variant="filledBlue" data-testid="contact-pay" disabled={offer.payBlocked !== null && !locked} onClick={pay}>
          Pay
        </DashButton>
      );
      break;
    case "ignored":
      note = "You ignored this contact request.";
      buttons = (
        <DashButton key="unignore" variant="tintedBlue" data-testid="contact-unignore" disabled={busy} onClick={() => void actions.unignore(c.contact).then(done)}>
          {actions.isRunning("unignore", c.contact) ? "Unignoring…" : "Unignore"}
        </DashButton>
      );
      break;
  }
  return (
    <div className="contact-pane-actions" data-testid="contact-pane-actions" ref={area}>
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

/** "Only visible to you" (D7): alias, note and Hide, saved and published together. A re-read of
 * the saved details updates the fields the user has not edited; Hide shows what is saved, or the
 * value being saved. */
function PrivateDetails({ walletId, contact: c, detail, actions }: { walletId: string; contact: ContactSummaryInfo; detail: ContactDetailInfo; actions: ContactActions }) {
  useModel(actions);
  const id = useId();
  const saved = detailsForm(detail.privateDetails);
  const [base, setBase] = useState(saved);
  const [text, setText] = useState({ alias: saved.alias, note: saved.note });
  if (base.alias !== saved.alias || base.note !== saved.note) {
    setBase(saved);
    setText((t) => ({ alias: t.alias === base.alias ? saved.alias : t.alias, note: t.note === base.note ? saved.note : t.note }));
  }
  const [hiding, setHiding] = useState<boolean | null>(null);
  // Once the re-read holds the value Hide saved, it is the saved one.
  if (hiding !== null && hiding === saved.hidden) setHiding(null);
  const form: DetailsForm = { ...text, hidden: hiding ?? saved.hidden };
  const busy = actions.busy;
  const saving = actions.isRunning("details", c.contact);
  const outcome = actions.outcome?.contact === c.contact && actions.outcome.kind === "details" ? actions.outcome : null;
  const toast = useActionToast(walletId);
  const save = (f: DetailsForm) =>
    actions.saveDetails(c.contact, detailsOf(f)).then((o) => {
      toast(o, true);
      return o;
    });
  return (
    <section className="contact-pane-section" aria-labelledby={`${id}-title`} data-testid="contact-private">
      <h3 id={`${id}-title`} className="contact-pane-heading">
        Only visible to you
      </h3>
      <form
        className="contact-details-form"
        onSubmit={(e) => {
          e.preventDefault();
          void save(form);
        }}
      >
        <label className="form-field">
          Alias
          <input data-testid="contact-alias" value={form.alias} disabled={busy} onChange={(e) => setText({ ...text, alias: e.target.value })} />
        </label>
        <label className="form-field">
          Note
          <textarea className="contact-note" data-testid="contact-note" rows={3} value={form.note} disabled={busy} onChange={(e) => setText({ ...text, note: e.target.value })} />
        </label>
        <label className="check-row contact-hide">
          <input
            type="checkbox"
            data-testid="contact-hide"
            checked={form.hidden}
            disabled={busy}
            onChange={(e) => {
              // Hide saves at once, with the alias and note as last saved (D7); a refusal or a
              // cancel shows the saved value again.
              const hidden = e.target.checked;
              setHiding(hidden);
              void save({ ...saved, hidden }).then((o) => {
                if (!o?.ok) setHiding(null);
              });
            }}
          />
          Hide this contact
        </label>
        <span className="add-contact-actions">
          <DashButton variant="tintedBlue" type="submit" data-testid="contact-details-save" disabled={busy || !detailsChanged(form, detail.privateDetails)}>
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
  const activity = useContactActivity(pane.walletId, pane.identity, contact, filter);
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
