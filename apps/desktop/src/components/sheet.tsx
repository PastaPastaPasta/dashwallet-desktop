// SheetScaffold (UX-SPEC C23): a modal dialog with the 64 pt header (title, close), Escape to
// close, focus kept inside while open and returned to the opener on close. Focus starts on the
// element marked `data-autofocus`, else on the first focusable one (the close button), and again
// whenever `focusKey` changes (a sheet with steps). `closable={false}` holds the sheet open while
// something it must report back is running.

import { useEffect, useRef, type ReactNode } from "react";

const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), select, textarea, summary, [tabindex]:not([tabindex="-1"])';

function focusStart(panel: HTMLElement | null) {
  (panel?.querySelector<HTMLElement>("[data-autofocus]") ?? panel?.querySelector<HTMLElement>(FOCUSABLE))?.focus();
}

export function Sheet({
  title,
  onClose,
  width,
  testId,
  focusKey,
  closable = true,
  children,
}: {
  title: string;
  onClose: () => void;
  width: "small" | "medium" | "large";
  testId: string;
  focusKey?: string;
  closable?: boolean;
  children: ReactNode;
}) {
  const panel = useRef<HTMLDivElement>(null);
  // The opener first: the effect below moves focus away from it.
  useEffect(() => {
    const opener = document.activeElement as HTMLElement | null;
    return () => opener?.focus?.();
  }, []);
  useEffect(() => focusStart(panel.current), [focusKey]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      if (closable) onClose();
      return;
    }
    if (e.key !== "Tab" || !panel.current) return;
    const items = [...panel.current.querySelectorAll<HTMLElement>(FOCUSABLE)];
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  };

  return (
    <div className="sheet-scrim" onMouseDown={(e) => closable && e.target === e.currentTarget && onClose()}>
      <div
        ref={panel}
        className={`sheet sheet-${width}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={`${testId}-title`}
        data-testid={testId}
        onKeyDown={onKeyDown}
      >
        <header className="sheet-header" data-testid={`${testId}-header`}>
          <span className="sheet-header-side" />
          <h2 id={`${testId}-title`} className="sheet-title">
            {title}
          </h2>
          <span className="sheet-header-side">
            <button type="button" className="sheet-close" aria-label="Close" disabled={!closable} onClick={onClose}>
              <svg viewBox="0 0 20 20" aria-hidden="true">
                <path d="M5 5l10 10M15 5 5 15" />
              </svg>
            </button>
          </span>
        </header>
        <div className="sheet-body">{children}</div>
      </div>
    </div>
  );
}
