// The dialogs the menus open: File ▸ Open URI… (QT-019) and About.

import { useId, useState, type FormEvent } from "react";
import { errorMessage } from "../api/backend";
import type { LaunchInfo } from "../api/generated/LaunchInfo";
import { useBackend } from "../api/queries";
import { applyLink, linkProblem } from "../app/shellBridge";
import { ui } from "../app/ui";
import { DashButton } from "../components/basics";
import { Sheet } from "../components/sheet";

const close = () => ui.set({ dialog: null });

/** Opens a `dash:`, `pay:`, `dashwallet:` or `dashpay:` link, routed as a clicked one is. */
export function OpenUriSheet({ network }: { network: string }) {
  const backend = useBackend();
  const id = useId();
  const [text, setText] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    try {
      const route = await backend.routeLink(network, text);
      const why = linkProblem(route);
      if (why) setProblem(why);
      else applyLink(route);
    } catch (err) {
      setProblem(errorMessage(err));
    }
  };
  return (
    <Sheet title="Open URI" width="small" testId="open-uri" onClose={close}>
      <form className="pay-form" onSubmit={(e) => void submit(e)}>
        <div className="form-field">
          <label htmlFor={id}>Payment request or link</label>
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
          Open
        </DashButton>
      </form>
    </Sheet>
  );
}

export function AboutSheet({ launch }: { launch: LaunchInfo }) {
  return (
    <Sheet title="About Dash Wallet" width="small" testId="about" onClose={close}>
      <dl className="about-list">
        <dt>Version</dt>
        <dd data-testid="about-version">{launch.appVersion}</dd>
        <dt>Engine</dt>
        <dd>{launch.engineVersion}</dd>
        <dt>Network</dt>
        <dd>{launch.network.dirName}</dd>
      </dl>
    </Sheet>
  );
}
