// The Contacts list (DP2-07): the engine's sections as groups (D1, D5), one button per row (D7,
// D13), and the network search under them (D6).

import { useId, useRef, type KeyboardEvent } from "react";
import type { UserHit } from "../../api/generated/UserHit";
import { Chip, DashButton } from "../../components/basics";
import { avatarLetters, initials, moveFocus, relationNote, rowChips, rowNote, shownName, subtitle, type Group } from "../../contacts/model";
import { Avatar } from "./Avatar";

export function ContactGroups({
  groups,
  selected,
  onSelect,
  onToggleHidden,
}: {
  groups: Group[];
  selected: string | null;
  onSelect: (id: string) => void;
  onToggleHidden: () => void;
}) {
  const list = useRef<HTMLDivElement>(null);
  // Arrows, Home and End move between the rows of the open groups, across groups (D13).
  const onKeyDown = (e: KeyboardEvent) => {
    const row = (e.target as HTMLElement).closest<HTMLElement>("[data-contact]");
    if (!row) return;
    const ids = groups.filter((g) => g.open).flatMap((g) => g.contacts.map((c) => c.contact));
    const next = moveFocus(ids, row.dataset.contact ?? null, e.key);
    if (next === null) return;
    e.preventDefault();
    list.current?.querySelector<HTMLElement>(`[data-contact="${CSS.escape(next)}"]`)?.focus();
  };
  return (
    <div className="contacts-groups" ref={list} onKeyDown={onKeyDown} data-testid="contacts-groups">
      {groups.map((g) => (
        <ContactGroup key={g.section} group={g} selected={selected} onSelect={onSelect} onToggle={onToggleHidden} />
      ))}
    </div>
  );
}

function ContactGroup({ group, selected, onSelect, onToggle }: { group: Group; selected: string | null; onSelect: (id: string) => void; onToggle: () => void }) {
  const titleId = useId();
  const rowsId = useId();
  return (
    <section className="contacts-group card-l1" aria-labelledby={titleId} data-testid={`contacts-group-${group.section}`}>
      <h2 className="contacts-group-title" id={titleId}>
        {group.collapsible ? (
          <button type="button" className="contacts-disclosure" aria-expanded={group.open} aria-controls={rowsId} onClick={onToggle}>
            {group.title}
          </button>
        ) : (
          group.title
        )}
      </h2>
      {group.open && (
        <ul className="contacts-rows" id={rowsId}>
          {group.contacts.map((c) => {
            const sub = subtitle(c);
            const note = rowNote(group.section, c);
            return (
              <li key={c.contact}>
                <button
                  type="button"
                  className="contact-row"
                  data-testid="contact-row"
                  data-contact={c.contact}
                  aria-current={selected === c.contact ? "true" : undefined}
                  onClick={() => onSelect(c.contact)}
                >
                  <Avatar letters={avatarLetters(c)} />
                  <span className="contact-row-text">
                    <span className="contact-row-name">{shownName(c)}</span>
                    {sub && <span className="contact-row-username">{sub}</span>}
                  </span>
                  <span className="contact-row-trailing">
                    {rowChips(c).map((chip) => (
                      <span key={chip.id} title={chip.detail} data-testid={`contact-chip-${chip.id}`}>
                        <Chip tone={chip.tone}>{chip.text}</Chip>
                      </span>
                    ))}
                    {note && <span className="contact-row-note">{note}</span>}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

/** "On the Dash network": the usernames the network search found that the list does not show (D6). */
export function NetworkResults({
  state,
  shown,
  more,
  onRetry,
  onPick,
  onMore,
}: {
  state: "loading" | "error" | "done";
  shown: UserHit[];
  more: boolean;
  onRetry: () => void;
  onPick: (hit: UserHit) => void;
  onMore: () => void;
}) {
  const titleId = useId();
  return (
    <section className="contacts-group card-l1 contacts-network" aria-labelledby={titleId} aria-busy={state === "loading"} data-testid="contacts-network">
      <h2 className="contacts-group-title" id={titleId}>
        On the Dash network
      </h2>
      {state === "loading" && (
        <p className="contacts-network-note" role="status">
          Searching…
        </p>
      )}
      {state === "error" && (
        <p className="contacts-network-note" role="alert" data-testid="contacts-network-error">
          Could not search the Dash network.{" "}
          <DashButton variant="plainBlue" size="small" onClick={onRetry}>
            Try again
          </DashButton>
        </p>
      )}
      {state === "done" && shown.length === 0 && (
        <p className="contacts-network-note" data-testid="contacts-network-none">
          No usernames found
        </p>
      )}
      {state === "done" && shown.length > 0 && (
        <ul className="contacts-rows">
          {shown.map((h) => {
            const note = relationNote(h);
            return (
              <li key={h.identity}>
                <button type="button" className="contact-row" data-testid="network-row" data-identity={h.identity} onClick={() => onPick(h)}>
                  <Avatar letters={initials(h.displayName?.trim() || h.username)} />
                  <span className="contact-row-text">
                    <span className="contact-row-name">{h.displayName?.trim() || h.username}</span>
                    {h.displayName?.trim() && <span className="contact-row-username">@{h.username}</span>}
                  </span>
                  <span className="contact-row-trailing">
                    {h.unverified && <Chip tone="warning">Unverified</Chip>}
                    {note && <span className="contact-row-note">{note}</span>}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
      {state === "done" && more && (
        <DashButton variant="plainBlue" size="small" className="contacts-more" data-testid="contacts-network-more" onClick={onMore}>
          Show more results
        </DashButton>
      )}
    </section>
  );
}
