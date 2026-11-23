// A contact request's Accept and Ignore beside its row (DP2-09 D15), in the Contacts list and in
// the bell; `testPrefix` names them (`<prefix>-accept`, `<prefix>-ignore`).

import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import { DashButton } from "../../components/basics";
import type { ActionOutcome, ContactActions } from "../../contacts/actions";
import { shownName } from "../../contacts/model";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";

export function RequestButtons({
  contact: c,
  actions,
  hasKeys,
  testPrefix,
  onDone,
}: {
  contact: ContactSummaryInfo;
  actions: ContactActions;
  hasKeys: boolean | undefined;
  testPrefix: string;
  /** Gets the outcome (`null`: cancelled, or another action ran). */
  onDone?: (outcome: ActionOutcome | null) => void;
}) {
  useModel(actions);
  const t = useT();
  const name = shownName(c);
  const accepting = actions.isRunning("accept", c.contact);
  const ignoring = actions.isRunning("ignore", c.contact);
  return (
    <span className="contact-row-actions">
      <DashButton
        variant="filledBlue"
        size="small"
        data-testid={`${testPrefix}-accept`}
        aria-label={accepting ? t("contacts.request.acceptingName", { name }) : t("contacts.request.acceptName", { name })}
        title={hasKeys === false ? t("contacts.pane.enableFirst") : undefined}
        disabled={actions.busy || hasKeys === false}
        onClick={() => void actions.accept(c.contact, name).then(onDone)}
      >
        {accepting ? t("contacts.request.accepting") : t("contacts.request.accept")}
      </DashButton>
      <DashButton
        variant="tintedGray"
        size="small"
        data-testid={`${testPrefix}-ignore`}
        aria-label={ignoring ? t("contacts.request.ignoringName", { name }) : t("contacts.request.ignoreName", { name })}
        disabled={actions.busy}
        onClick={() => void actions.ignore(c.contact).then(onDone)}
      >
        {ignoring ? t("contacts.request.ignoring") : t("contacts.request.ignore")}
      </DashButton>
    </span>
  );
}
