// SheetScaffold (UX-SPEC C23): a modal dialog with the 64 pt header (title, close), Escape to
// close, focus kept inside while open and returned to the opener on close. Focus starts on the
// element marked `data-autofocus`, else on the first focusable one (the close button), and again
// whenever `focusKey` changes (a sheet with steps). `closable={false}` holds the sheet open while
// something it must report back is running.
//
// A sheet opened from inside the shell (Pay, Receive) renders into App's sheet layer, above the
// shell and below the lock screen, and the shell goes inert while it is open (U-03 D22).

import { createContext, useContext, useEffect, useRef, useSyncExternalStore, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { DashButton } from "./basics";

/** App's layer for sheets opened inside the shell; `null` renders a sheet in place. */
export const SheetLayer = createContext<HTMLElement | null>(null);

let layered = 0;
const watchers = new Set<() => void>();
function hold(by: number) {
  layered += by;
  for (const w of watchers) w();
}

/** Whether a sheet is open in the layer: the shell behind it takes no focus. */
export function useLayeredSheet(): boolean {
  return useSyncExternalStore(
    (w) => {
      watchers.add(w);
      return () => void watchers.delete(w);
    },
    () => layered > 0,
  );
}

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
  returnFocus,
  children,
}: {
  title: string;
  onClose: () => void;
  width: "small" | "medium" | "large";
  testId: string;
  focusKey?: string;
  closable?: boolean;
  /** Where focus goes on close, instead of the element focused at open (gone, or `<body>`
   * when one sheet replaced another). */
  returnFocus?: () => HTMLElement | null;
  children: ReactNode;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const layer = useContext(SheetLayer);
  useEffect(() => {
    if (!layer) return;
    hold(1);
    return () => hold(-1);
  }, [layer]);
  useEffect(() => {
    // The opener first: the effect below moves focus away from it.
    const opener = document.activeElement as HTMLElement | null;
    return () => {
      const back = () => {
        const to = returnFocus?.() ?? opener;
        if (to?.isConnected) to.focus();
      };
      if (!layer) return back();
      // In the layer the shell is still inert now, so focusing into it would do nothing: once it is
      // not, return focus if it was lost (not if another sheet took it).
      requestAnimationFrame(() => {
        if (!document.activeElement || document.activeElement === document.body) back();
      });
    };
    // The opener and the return target are read when the sheet opens and closes.
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

  const sheet = (
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
  return layer ? createPortal(sheet, layer) : sheet;
}

/** A question with Cancel and one confirming button; `onConfirm` runs after the sheet has closed. */
export function QuestionSheet({
  title,
  text,
  confirm,
  testId,
  confirmTestId,
  onConfirm,
  onClose,
}: {
  title: string;
  text: string;
  confirm: string;
  testId: string;
  confirmTestId: string;
  onConfirm: () => void;
  onClose: () => void;
}) {
  return (
    <Sheet title={title} width="small" testId={testId} onClose={onClose}>
      <p className="sheet-text">{text}</p>
      <div className="sheet-actions">
        <DashButton variant="tintedGray" data-autofocus onClick={onClose}>
          Cancel
        </DashButton>
        <DashButton
          variant="filledBlue"
          data-testid={confirmTestId}
          onClick={() => {
            onClose();
            onConfirm();
          }}
        >
          {confirm}
        </DashButton>
      </div>
    </Sheet>
  );
}
