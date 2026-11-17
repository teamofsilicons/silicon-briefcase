/**
 * The app's logo as an SVG document, for the places that need a file rather than a component: the home-screen icon
 * (app/apple-icon.tsx, on a full square the system rounds itself) and the Open Graph image (app/og.png). The logo is
 * lib/app-logo.ts `LOGO`: the mark in #181818 on a grey square, as Silicon Accounts draws its own icon. The browser-tab
 * icon is app/icon.svg: the mark alone with no tile, in ink on a light browser and grey on a dark one.
 */
import { LOGO } from "./app-logo";

export const ICON_PAPER = "#E3E3E3";
export const ICON_INK = "#181818";

/** The logo, drawn as it is. */
export function markSvg(): string {
  return LOGO;
}

export const svgDataUri = (svg: string) => `data:image/svg+xml;base64,${Buffer.from(svg, "utf8").toString("base64")}`;
