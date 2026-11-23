// Pay ▸ To an address ▸ Scan QR (IOS-043 on the desktop: no camera). An image file, a pasted or
// dropped image, or a screen region is decoded; its codes are read in order as payment links,
// a bare address as `dash:<address>`, and the first that pays opens Pay prefilled, as a `dash:`
// link does (`applyLink`). A code that is a link of another kind says why, as Open URI does.

import type { Backend } from "../../api/backend";
import type { LinkRoute } from "../../api/generated/LinkRoute";
import { imageError, MAX_IMAGE_BYTES } from "../../contacts/add";
import { bytesToBase64 } from "../../app/files";
import { t } from "../../i18n";
import { Model } from "../../onboarding/model";
import { linkProblem } from "../../app/shellBridge";

export type PayScanApi = Pick<Backend, "scanQrFile" | "scanQrImage" | "scanQrScreen" | "cancelScreenScan" | "routeLink">;

export type PayScanState = { kind: "idle" } | { kind: "reading" } | { kind: "capturing" } | { kind: "failed"; text: string };

/** A code as Pay reads it: a link as it is, anything else as a bare address. */
export const asLink = (text: string) => (/^[a-z][a-z0-9+.-]*:/i.test(text) ? text : `dash:${text}`);

class TooLarge extends Error {}

export class PayScanModel extends Model {
  state: PayScanState = { kind: "idle" };
  #seq = 0;

  constructor(
    private readonly api: PayScanApi,
    private readonly network: string,
    private readonly onPay: (route: Extract<LinkRoute, { kind: "pay" }>) => void,
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
    if (file.size > MAX_IMAGE_BYTES) return Promise.resolve(this.#fail(t("contacts.scan.imageTooLarge")));
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
    void this.api.cancelScreenScan().catch(() => undefined);
    this.#seq++;
    this.#go({ kind: "idle" });
  }

  dismiss() {
    if (this.state.kind === "failed") this.#go({ kind: "idle" });
  }

  async #read(kind: "reading" | "capturing", read: () => Promise<string[] | null>) {
    if (this.state.kind === "capturing") void this.api.cancelScreenScan().catch(() => undefined);
    const seq = ++this.#seq;
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
      const trimmed = text.trim();
      if (!trimmed) continue;
      let route: LinkRoute;
      try {
        route = await this.api.routeLink(this.network, asLink(trimmed));
      } catch (e) {
        if (seq !== this.#seq) return;
        problem ??= imageError(e);
        continue;
      }
      if (seq !== this.#seq) return;
      if (route.kind === "pay") {
        this.#go({ kind: "idle" });
        this.onPay(route);
        return;
      }
      // A payment link that can't be paid says why; any other code is just not a payment.
      const payLink = /^(dash|pay):/i.test(trimmed);
      if (route.kind === "unsupported" || (payLink && route.kind === "rejected")) problem ??= linkProblem(route);
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
