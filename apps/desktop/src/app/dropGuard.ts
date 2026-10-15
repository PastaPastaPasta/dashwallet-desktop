// DP2-08 D11: Tauri's own drop handler is off (`dragDropEnabled: false`), so the webview gets
// HTML5 drops. A drop area (Add Contact's Scan) handles its own and cancels the default; anywhere
// else the document refuses the drop, so a dropped file or link never navigates the webview.

export function installDropGuard(doc: Document = document) {
  doc.addEventListener("dragover", (e) => {
    if (e.defaultPrevented) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "none";
  });
  doc.addEventListener("drop", (e) => e.preventDefault());
}
