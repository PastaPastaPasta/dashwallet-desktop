// Small building blocks (UX-SPEC §3.1): icons, the Dash glyph and wordmark, AmountText (C10),
// chips and the network capsule (C20) and DashButton (C14). Styles are in styles/app.css and read only --dw-* tokens.

import type { ButtonHTMLAttributes, CSSProperties, ReactNode, Ref } from "react";
import { useAppearance } from "../app/theme";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { iconFiles, type IconFiles, type UsedIcon } from "./icons";

export function Icon({ id, size, className }: { id: UsedIcon; size: number; className?: string }) {
  const appearance = useAppearance();
  const files: IconFiles = iconFiles[id];
  const src = appearance === "dark" && files.dark ? files.dark : files.light;
  return <img className={className} src={src} width={size} height={size} alt="" aria-hidden="true" draggable={false} />;
}

/** The Dash wordmark as a template, tinted by the `.dash-logo` rule's colour. */
export function DashLogo({ className }: { className: string }) {
  const style = { "--logo-url": `url("${iconFiles.dashLogo.light}")` } as CSSProperties;
  return <span className={`dash-logo ${className}`} style={style} role="img" aria-label="Dash" />;
}

/** The orange capsule naming a non-mainnet network (Overview hero, lock screen). */
export function NetworkCapsule({ network, testId }: { network: NetworkInfo; testId?: string }) {
  if (network.kind === "mainnet") return null;
  return (
    <span className="network-capsule" data-testid={testId}>
      {(network.kind === "devnet" ? network.dirName : network.kind).toUpperCase()}
    </span>
  );
}

/** The Dash currency glyph, a template image tinted with the text colour. */
export function Glyph({ factor = 1 }: { factor?: number }) {
  const style = { "--glyph-url": `url("${iconFiles.dashCurrency.light}")`, "--glyph-factor": factor } as CSSProperties;
  return <span className="glyph" style={style} aria-hidden="true" />;
}

export function VisuallyHidden({ children }: { children: ReactNode }) {
  return <span className="sr-only">{children}</span>;
}

/**
 * `[sign][number]` then the Dash glyph (DASH/tDASH) or a space and the unit name. The view
 * never formats numbers; callers pass the formatted string. The unit name is always spoken
 * and copied, so testnet units stay visible to assistive technology (UX-SPEC §5.1).
 */
export function AmountText({
  text,
  unitName,
  glyph = true,
  glyphFactor = 0.7,
  className,
  title,
}: {
  text: string;
  unitName: string;
  glyph?: boolean;
  glyphFactor?: number;
  className?: string;
  title?: string;
}) {
  return (
    <span className={`amount ${className ?? ""}`} title={title}>
      <span className="amount-number">{text}</span>
      {glyph ? (
        <>
          <Glyph factor={glyphFactor} />
          <VisuallyHidden> {unitName}</VisuallyHidden>
        </>
      ) : (
        <span className="amount-unit"> {unitName}</span>
      )}
    </span>
  );
}

export type Tone = "info" | "danger";

export function Chip({ tone = "info", children }: { tone?: Tone; children: ReactNode }) {
  return <span className={`chip chip-${tone}`}>{children}</span>;
}

// The styles and sizes the gate screens use; the rest of DashUIKit's catalogue comes with the screens that need it.
export type ButtonStyle = "plainBlue" | "plainRed" | "plainWhite" | "filledWhiteBlue" | "filledBlue" | "tintedBlue" | "tintedGray";
export type ButtonSize = "large" | "medium" | "small";

export function DashButton({
  variant,
  size = "medium",
  fill = false,
  className,
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant: ButtonStyle; size?: ButtonSize; fill?: boolean; ref?: Ref<HTMLButtonElement> }) {
  return (
    <button
      type="button"
      {...rest}
      className={`dash-button dash-button-${variant} dash-button-${size} ${fill ? "dash-button-fill" : ""} ${className ?? ""}`}
    >
      {children}
    </button>
  );
}
