// The head of a Home pool card (Shielded, Platform): its title stays a heading. While the pool can
// be moved from, the title is a button that opens Transfer from it, and its hit area covers the head.

import { useId, type ReactNode } from "react";
import { VisuallyHidden } from "../components/basics";
import { TRANSFER_COPY } from "../transfer/model";

export function CardHead({ title, text, textTestId, buttonTestId, onTransfer }: { title: string; text: ReactNode; textTestId?: string; buttonTestId: string; onTransfer: (() => void) | null }) {
  const textId = useId();
  if (!onTransfer)
    return (
      <>
        <h2 className="platform-card-title">{title}</h2>
        <p className="platform-card-text" data-testid={textTestId}>
          {text}
        </p>
      </>
    );
  return (
    <div className="card-head">
      <h2 className="platform-card-title">
        <button type="button" className="card-transfer" data-testid={buttonTestId} aria-describedby={textId} onClick={onTransfer}>
          {title}
          <VisuallyHidden>{TRANSFER_COPY.cardButton}</VisuallyHidden>
          <span className="card-transfer-chevron" aria-hidden="true">
            {" ›"}
          </span>
        </button>
      </h2>
      <p className="platform-card-text" id={textId} data-testid={textTestId}>
        {text}
      </p>
    </div>
  );
}
