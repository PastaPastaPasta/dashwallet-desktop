// SheetScaffold (UX-SPEC C23): a modal dialog with the 64 pt header (title, close), Escape to
// close, focus kept inside while open and returned to the opener on close. Focus starts on the
// element marked `data-autofocus`, else on the first focusable one (the close button).
//
// A sheet opened from inside the shell (Pay, Receive) renders into App's sheet layer, above the
// shell and below the lock screen, and the shell goes inert while it is open (U-03 D22).

import { createContext, useContext, useEffect, useRef, useSyncExternalStore, type ReactNode } from "react";
import { createPortal } from "react-dom";

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

export function Sheet({
  title,
  onClose,
  width,
  testId,
  children,
}: {
  title: string;
  onClose: () => void;
  width: "small" | "medium" | "large";
  testId: string;
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
    const opener = document.activeElement as HTMLElement | null;
    const start = panel.current?.querySelector<HTMLElement>("[data-autofocus]") ?? panel.current?.querySelector<HTMLElement>(FOCUSABLE);
    start?.focus();
    return () => opener?.focus?.();
  }, []);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose();
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
    <div className="sheet-scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
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
            <button type="button" className="sheet-close" aria-label="Close" onClick={onClose}>
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
