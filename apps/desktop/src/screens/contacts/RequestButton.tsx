// The contact-request card's own actions, shared by Add contact's card and Pay's found user:
// Send Contact Request and Enable through `ContactActions`, and Enable's refusal.

import { DashButton } from "../../components/basics";
import type { ActionOutcome, ContactActions } from "../../contacts/actions";
import { Outcome } from "../tools/common";

type Size = "small" | "medium";

export function SendRequestButton({
  actions,
  contact,
  name,
  scan = null,
  size,
  testId,
  onDone,
}: {
  actions: ContactActions | null;
  contact: string;
  name: string;
  scan?: string | null;
  size?: Size;
  testId: string;
  onDone: (o: ActionOutcome | null) => void;
}) {
  return (
    <DashButton variant="filledBlue" size={size} data-testid={testId} disabled={!actions || actions.busy} onClick={() => void actions?.send(contact, name, scan).then(onDone)}>
      {actions?.isRunning("send", contact) ? "Sending…" : "Send Contact Request"}
    </DashButton>
  );
}

export function EnableButton({ actions, size, testId, onDone }: { actions: ContactActions | null; size?: Size; testId: string; onDone: (o: ActionOutcome | null) => void }) {
  return (
    <DashButton variant="tintedBlue" size={size} data-testid={testId} disabled={!actions || actions.busy} onClick={() => void actions?.enable().then(onDone)}>
      {actions?.isRunning("enable", null) ? "Enabling…" : "Enable"}
    </DashButton>
  );
}

/** Enable's refusal; its success shows as the card's next step (Send). */
export function EnableFailure({ actions, testId }: { actions: ContactActions | null; testId: string }) {
  const o = actions?.outcome?.kind === "enable" && !actions.outcome.ok ? actions.outcome : null;
  return o && <Outcome ok={false} text={o.text} testId={testId} />;
}
