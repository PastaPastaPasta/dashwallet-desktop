// Pay ▸ To an address ▸ Scan QR (IOS-043 on the desktop: no camera). An image file, a pasted or
// dropped image, or a screen region is decoded, and its codes are read in order. A `dash:` or
// `pay:` link goes through the link router; a bare code is classified as a typed address is
// (Core or Platform). The first that pays fills the open form in place: address, and a link's
// amount, label and message. Other codes are skipped without side effects (an invitation is not
// stashed). Nothing is sent: Review and Confirm still run.

import type { Backend } from "../../api/backend";
import type { PaymentUri } from "../../api/generated/PaymentUri";
import { imageError, MAX_IMAGE_BYTES } from "../../contacts/add";
import { bytesToBase64 } from "../../app/files";
import { t } from "../../i18n";
import { Model } from "../../onboarding/model";
import { linkProblem } from "../../app/shellBridge";

export type PayScanApi = Pick<Backend, "scanQrFile" | "scanQrImage" | "scanQrScreen" | "cancelScreenScan" | "routeLink" | "classifyAddress">;

export type PayScanState = { kind: "idle" } | { kind: "reading" } | { kind: "capturing" } | { kind: "failed"; text: string };

/** What a scan puts in the form. */
export type Scanned = Pick<PaymentUri, "address" | "amount" | "label" | "message">;

const PAY_LINK = /^(dash|pay):/i;
const ANY_LINK = /^[a-z][a-z0-9+.-]*:/i;

class TooLarge extends Error {}

export class PayScanModel extends Model {
  state: PayScanState = { kind: "idle" };
  #seq = 0;

  constructor(
    private readonly api: PayScanApi,
    private readonly network: string,
    private readonly onScanned: (scanned: Scanned) => void,
  ) {
    super();
  }

  get busy() {
    return this.state.kind === "reading" || this.state.kind === "capturing";
  }

  /** Open Image…; a cancelled dialog changes nothing. */
  openImage(): Promise<void> {
    return this.#read("reading", () => this.api.scanQrFile());
  }

  /** A pasted or dropped image file; one too large is refused unread. */
  imageFile(file: Pick<Blob, "size" | "arrayBuffer">): Promise<void> {
    if (file.size > MAX_IMAGE_BYTES) {
      this.#stop();
      this.#fail(t("contacts.scan.imageTooLarge"));
      return Promise.resolve();
    }
    return this.#read("reading", async () => {
      const buffer = await file.arrayBuffer();
      if (buffer.byteLength > MAX_IMAGE_BYTES) throw new TooLarge(t("contacts.scan.imageTooLarge"));
      return this.api.scanQrImage(bytesToBase64(new Uint8Array(buffer)));
    });
  }

  /** Scan a screen region; the user's cancel changes nothing. */
  screen(): Promise<void> {
    return this.#read("capturing", () => this.api.scanQrScreen());
  }

  cancelScreen() {
    if (this.state.kind !== "capturing") return;
    this.#stop();
    this.#go({ kind: "idle" });
  }

  /** The user edited the address: a scan under way no longer applies, nor its refusal. */
  dismiss() {
    if (this.state.kind === "idle") return;
    this.#stop();
    this.#go({ kind: "idle" });
  }

  /** The form went away: nothing pending may fill it, and a picker still open is closed. */
  dispose() {
    this.#stop();
  }

  /** Drops the request under way (a waiting screen capture too). */
  #stop(): number {
    if (this.state.kind === "capturing") void this.api.cancelScreenScan().catch(() => undefined);
    return ++this.#seq;
  }

  async #read(kind: "reading" | "capturing", read: () => Promise<string[] | null>) {
    const seq = this.#stop();
    this.#go({ kind });
    let texts: string[] | null;
    try {
      texts = await read();
    } catch (e) {
      if (seq === this.#seq) this.#fail(e instanceof TooLarge ? e.message : imageError(e));
      return;
    }
    if (seq !== this.#seq) return;
    if (texts === null) return this.#go({ kind: "idle" });
    let problem: string | null = null;
    for (const text of texts) {
      const code = text.trim();
      if (!code) continue;
      let scanned: Scanned | null = null;
      try {
        if (PAY_LINK.test(code)) {
          const route = await this.api.routeLink(this.network, code);
          if (route.kind === "pay") scanned = route.uri;
          // A payment link that can't be paid says why.
          else if (route.kind === "unsupported" || route.kind === "rejected") problem ??= linkProblem(route);
        } else if (!ANY_LINK.test(code)) {
          const kind = (await this.api.classifyAddress(this.network, code)).kind;
          if (kind === "core" || kind === "platform") scanned = { address: code, amount: null, label: null, message: null };
        }
      } catch (e) {
        if (seq !== this.#seq) return;
        problem ??= imageError(e);
        continue;
      }
      if (seq !== this.#seq) return;
      if (scanned) {
        this.#go({ kind: "idle" });
        this.onScanned(scanned);
        return;
      }
    }
    this.#fail(problem ?? t("pay.scan.noPayment"));
  }

  #fail(text: string) {
    this.#go({ kind: "failed", text });
  }

  #go(state: PayScanState) {
    this.state = state;
    this.changed();
  }
}
