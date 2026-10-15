// DP2-08 D11: Tauri's own drop handler is off (`dragDropEnabled: false`), so the webview gets
// HTML5 drops. A drop area (Add Contact's Scan) handles its own and cancels the default; a text
// field takes a dragged text as typing it would (an address into Pay); anywhere else the document
// refuses the drop, so a dropped file or link never navigates the webview.

/** A text field, getting a drag without files. */
function intoTextField(e: DragEvent): boolean {
  if ([...(e.dataTransfer?.types ?? [])].includes("Files")) return false;
  const t = e.target as { tagName?: string; type?: string; isContentEditable?: boolean } | null;
  return !!t && (t.isContentEditable === true || t.tagName === "TEXTAREA" || (t.tagName === "INPUT" && t.type !== "file"));
}

export function installDropGuard(doc: Document = document) {
  doc.addEventListener("dragover", (e) => {
    if (e.defaultPrevented || intoTextField(e)) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "none";
  });
  doc.addEventListener("drop", (e) => {
    if (!intoTextField(e)) e.preventDefault();
  });
}
