// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// dw-app's QR scan commands (api/scan.rs, DP2-08 D9, D10) for the fixture. It decodes no image:
// a fixture "image" is a text file starting `fixture-qr:`, then one QR code's text per line
// (`fixtureQrImage`). Anything else is `app.image_unreadable`, as a file that is not a PNG, JPEG
// or BMP is; `fixture-qr:` with no line is `app.no_qr_code`.

import type { AppError } from "../api/generated/commands";
import { base64ToBytes } from "../app/files";
import type { ScreenScanMode } from "../api/generated/ScreenScanMode";
import type { Handlers } from "./handlers";

const MAGIC = "fixture-qr:";

export interface ScanWorld {
  /** `screen_scan_mode`: Linux and Windows pick, macOS copies with the shortcut. */
  mode: ScreenScanMode;
  /** What the next `scan_qr_screen` gets: an image (base64), the user's cancel (`null`), or a
   * session without a picker. */
  screen: string | null | "unavailable";
  /** `scan_qr_screen` calls that are waiting; `cancel_screen_scan` ends them with `null`. */
  waiting: (() => void)[];
  /** Hold `scan_qr_screen` until `cancel_screen_scan` (the user still picking). */
  holdScreen: boolean;
}

export const scanWorld = (): ScanWorld => ({ mode: "picker", screen: null, waiting: [], holdScreen: false });

/** A fixture image of QR codes with `texts`, as raw bytes. */
export function fixtureQrImage(...texts: string[]): Uint8Array {
  return new TextEncoder().encode([MAGIC, ...texts].join("\n"));
}

const fail = (code: AppError["code"], message: string): AppError => ({ code, message, data: null });

/** dw-desktop's `qr::decode` for fixture images. */
export function decodeFixtureImage(bytes: Uint8Array): string[] {
  const text = new TextDecoder().decode(bytes);
  if (!text.startsWith(MAGIC)) throw fail("app.image_unreadable", "image unreadable: unknown image format");
  const texts = text.slice(MAGIC.length).split("\n").filter(Boolean);
  if (texts.length === 0) throw fail("app.no_qr_code", "no QR code found");
  return texts;
}

export function scanHandlers(): Handlers {
  return {
    screen_scan_mode: (_, { world }) => world.scan.mode,
    // The dialog as `open_file` records it (tools.ts), then the decoder.
    scan_qr_file: (_, { world }) => {
      const { files } = world.tools;
      files.dialogs.push({ purpose: "qrImage", save: false, suggestedName: null });
      return files.open ? decodeFixtureImage(base64ToBytes(files.open.data)) : null;
    },
    scan_qr_image: ({ data }) => {
      let bytes: Uint8Array;
      try {
        bytes = base64ToBytes(data);
      } catch {
        throw fail("invalid_argument", "data: not base64");
      }
      return decodeFixtureImage(bytes);
    },
    // Answers later when held (a promise: the mock IPC awaits it).
    scan_qr_screen: (_, { world }) => {
      const { scan } = world;
      const { screen } = scan;
      if (scan.mode !== "picker" || screen === "unavailable")
        throw fail("app.screen_capture_unavailable", "screen capture: org.freedesktop.portal.Desktop was not provided by any .service files unsupported here");
      if (!scan.holdScreen) return screen === null ? null : decodeFixtureImage(base64ToBytes(screen));
      return new Promise<string[] | null>((resolve) => scan.waiting.push(() => resolve(null))) as unknown as string[] | null;
    },
    cancel_screen_scan: (_, { world }) => {
      for (const done of world.scan.waiting.splice(0)) done();
      return null;
    },
  };
}
