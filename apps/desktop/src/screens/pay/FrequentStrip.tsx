// The frequent-contacts strip (DP3-06 D3, DP3-07 D7): the contacts the main identity pays most,
// each an avatar (DP4-03) and its name. Pay ▸ To a contact and Home show it; the caller decides
// what a pick does (`onPick`).

import { useId } from "react";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import { Avatar } from "../contacts/Avatar";
import { payeeOf, type Payee } from "./contact";

export function FrequentStrip({
  contacts,
  onPick,
  testId,
  className,
}: {
  contacts: ContactSummaryInfo[];
  onPick: (payee: Payee) => void;
  /** The section's test id; each chip's is it with `-chip`. */
  testId: string;
  className?: string;
}) {
  const id = useId();
  if (contacts.length === 0) return null;
  return (
    <section className={["pay-frequent", className].filter(Boolean).join(" ")} aria-labelledby={id} data-testid={testId}>
      <h2 className="contacts-group-title" id={id}>
        Frequent contacts
      </h2>
      <ul className="pay-frequent-chips">
        {contacts.map((c) => {
          const p = payeeOf(c);
          return (
            <li key={c.contact}>
              <button type="button" className="pay-frequent-chip" data-testid={`${testId}-chip`} data-contact={c.contact} onClick={() => onPick(p)}>
                <Avatar letters={p.letters} size="pane" identity={p.contact} />
                <span className="pay-frequent-name">{p.name}</span>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
