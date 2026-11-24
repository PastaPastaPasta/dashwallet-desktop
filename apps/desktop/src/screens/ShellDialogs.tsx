// The dialogs the menus open: File ▸ Open URI… (QT-019) and About.

import { useId, useState, type FormEvent } from "react";
import type { LaunchInfo } from "../api/generated/LaunchInfo";
import { useBackend } from "../api/queries";
import { openLinkText } from "../app/shellBridge";
import { ui } from "../app/ui";
import { DashButton } from "../components/basics";
import { Sheet } from "../components/sheet";
import { useT } from "../i18n";

const close = () => ui.set({ dialog: null });

/** Opens a `dash:`, `pay:`, `dashwallet:` or `dashpay:` link, routed as a clicked one is. */
export function OpenUriSheet({ network }: { network: string }) {
  const tr = useT();
  const backend = useBackend();
  const id = useId();
  const [text, setText] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const why = await openLinkText(backend, network, text);
    if (why) setProblem(why);
  };
  return (
    <Sheet title={tr("app.openUri.title")} width="small" testId="open-uri" onClose={close}>
      <form className="pay-form" onSubmit={(e) => void submit(e)}>
        <div className="form-field">
          <label htmlFor={id}>{tr("app.openUri.field")}</label>
          <input
            id={id}
            className="mono"
            data-testid="open-uri-input"
            data-autofocus
            value={text}
            placeholder="dash:"
            aria-describedby={problem ? `${id}-problem` : undefined}
            aria-invalid={problem ? true : undefined}
            onChange={(e) => {
              setText(e.target.value);
              setProblem(null);
            }}
          />
        </div>
        {problem && (
          <p id={`${id}-problem`} className="form-problem" role="alert" data-testid="open-uri-problem">
            {problem}
          </p>
        )}
        <DashButton variant="filledWhiteBlue" type="submit" disabled={!text.trim()} data-testid="open-uri-open">
          {tr("app.openUri.open")}
        </DashButton>
      </form>
    </Sheet>
  );
}

export function AboutSheet({ launch }: { launch: LaunchInfo }) {
  const tr = useT();
  return (
    <Sheet title={tr("app.about.title")} width="small" testId="about" onClose={close}>
      <dl className="about-list">
        <dt>{tr("app.about.version")}</dt>
        <dd data-testid="about-version">{launch.buildVersion}</dd>
        <dt>{tr("app.about.engine")}</dt>
        <dd>{launch.engineVersion}</dd>
        <dt>{tr("app.about.network")}</dt>
        <dd>{launch.network.dirName}</dd>
      </dl>
    </Sheet>
  );
}
