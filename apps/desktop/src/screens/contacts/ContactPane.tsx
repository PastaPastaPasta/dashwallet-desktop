// The Contacts detail slot (DP2-07 D11, D12): a `dashpay://user` link's card (U-01), else Add
// contact's placeholder (DP2-08), else the selected contact as the list knows it (the actions
// are DP2-09's), else a prompt to pick one.

import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import { DashButton } from "../../components/basics";
import { ui, type AddContactIntent, type ContactPrefill } from "../../app/ui";
import { avatarLetters, channelTitle, relationTitle, rowChips, shownName, subtitle } from "../../contacts/model";
import { detailDate } from "../../format/tx";
import { RowChips } from "./ContactList";
import { Avatar } from "./Avatar";

export function ContactPane({ prefill, add, selected }: { prefill: ContactPrefill | null; add: AddContactIntent | null; selected: ContactSummaryInfo | null }) {
  let slot;
  if (prefill) slot = <LinkCard prefill={prefill} />;
  else if (add) slot = <AddContact add={add} />;
  else if (selected) slot = <Selected contact={selected} />;
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

/** The selected contact, from its row (D11). */
function Selected({ contact: c }: { contact: ContactSummaryInfo }) {
  const sub = subtitle(c);
  const chips = rowChips(c);
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
        <dd className="mono" data-testid="contact-detail-identity">
          {c.contact}
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
      <p className="placeholder-note" data-testid="placeholder-contact-pane">
        Contact actions, activity and private details are not available yet.
      </p>
    </section>
  );
}
