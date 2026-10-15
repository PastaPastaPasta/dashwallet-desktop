// The Contacts list (DP2-07): the engine's sections as groups (D1, D5), one button per row (D7,
// D13), and the network search under them (D6).

import { useId, useRef, type ButtonHTMLAttributes, type KeyboardEvent, type ReactNode } from "react";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import type { UserHit } from "../../api/generated/UserHit";
import { Chip, DashButton } from "../../components/basics";
import { avatarLetters, initials, moveFocus, rowChips, rowNote, shownName, subtitle, type Group, type RowChip } from "../../contacts/model";
import { Avatar } from "./Avatar";

/** A row's chips, each with what it means as its tooltip. */
export function RowChips({ chips, testIds = false }: { chips: RowChip[]; testIds?: boolean }) {
  return chips.map((chip) => (
    <span key={chip.id} title={chip.detail} data-testid={testIds ? `contact-chip-${chip.id}` : undefined}>
      <Chip tone={chip.tone}>{chip.text}</Chip>
    </span>
  ));
}

/** What a row offers beside its button: a request's Accept and Ignore (DP2-09 D15). */
type RowActions = (c: ContactSummaryInfo) => ReactNode;

/** One row: avatar, name, `@username` under it, and what trails it; `after` sits beside the
 * row's button, as buttons do not nest (DP2-09 D15). */
function Row({
  letters,
  name,
  sub,
  trailing,
  after,
  ...button
}: { letters: string; name: string; sub: string | null; trailing: ReactNode; after?: ReactNode } & ButtonHTMLAttributes<HTMLButtonElement> &
  Record<`data-${string}`, string | undefined>) {
  return (
    <li className={after ? "contact-row-item has-actions" : undefined}>
      <button type="button" className="contact-row tx-row" {...button}>
        <Avatar letters={letters} />
        <span className="contact-row-text tx-row-text">
          <span className="contact-row-name tx-row-title">{name}</span>
          {sub && <span className="contact-row-username">{sub}</span>}
        </span>
        <span className="contact-row-trailing">{trailing}</span>
      </button>
      {after}
    </li>
  );
}

export function ContactGroups({
  groups,
  selected,
  onSelect,
  onToggleHidden,
  rowActions,
}: {
  groups: Group[];
  selected: string | null;
  onSelect: (id: string) => void;
  onToggleHidden: () => void;
  rowActions?: RowActions;
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
    <div className="tx-groups" ref={list} onKeyDown={onKeyDown} data-testid="contacts-groups">
      {groups.map((g) => (
        <ContactGroup key={g.section} group={g} selected={selected} onSelect={onSelect} onToggle={onToggleHidden} rowActions={rowActions} />
      ))}
    </div>
  );
}

function ContactGroup({
  group,
  selected,
  onSelect,
  onToggle,
  rowActions,
}: {
  group: Group;
  selected: string | null;
  onSelect: (id: string) => void;
  onToggle: () => void;
  rowActions?: RowActions;
}) {
  const titleId = useId();
  const rowsId = useId();
  return (
    <section className="tx-group card-l1" aria-labelledby={titleId} data-testid={`contacts-group-${group.section}`}>
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
        <ul className="tx-group-rows" id={rowsId}>
          {group.contacts.map((c) => {
            const note = rowNote(c);
            return (
              <Row
                key={c.contact}
                letters={avatarLetters(c)}
                name={shownName(c)}
                sub={subtitle(c)}
                trailing={
                  <>
                    <RowChips chips={rowChips(c)} testIds />
                    {note && <span className="contact-row-note">{note}</span>}
                  </>
                }
                data-testid="contact-row"
                data-contact={c.contact}
                aria-current={selected === c.contact ? "true" : undefined}
                onClick={() => onSelect(c.contact)}
                after={rowActions?.(c)}
              />
            );
          })}
        </ul>
      )}
    </section>
  );
}

/** "On the Dash network": the strangers the network search found (D6). */
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
    <section className="tx-group card-l1" aria-labelledby={titleId} aria-busy={state === "loading"} data-testid="contacts-network">
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
        <ul className="tx-group-rows">
          {shown.map((h) => {
            const displayName = h.displayName?.trim();
            return (
              <Row
                key={h.identity}
                letters={initials(displayName || h.username)}
                name={displayName || h.username}
                sub={displayName ? `@${h.username}` : null}
                trailing={h.unverified && <Chip tone="warning">Unverified</Chip>}
                data-testid="network-row"
                data-identity={h.identity}
                onClick={() => onPick(h)}
              />
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
