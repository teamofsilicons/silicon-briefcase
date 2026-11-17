/**
 * GET /og.png: the app's Open Graph image (1200 by 630), in Silicon Accounts' look: the mark on its grey square and
 * "Team of Silicons Briefcase", the landing page's headline in ink at the title weight, and its tagline, on the
 * neutral page colour. Everything comes from lib/app.config.ts, so it is drawn once at build time. The faces are
 * read from assets/og (TrueType: next/og cannot read the site's WOFF2 files; SIL OFL 1.1).
 */
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { ImageResponse } from "next/og";
import { appConfig } from "@/lib/app.config";
import { markSvg, svgDataUri } from "@/lib/brand-svg";

export const dynamic = "force-static";

const PAPER = "#FCFCFC";
const INK = "#1B1B1B";
const SECONDARY = "#505050";
const MUTED = "#6B6B6B";

const fonts = (async () => {
  const dir = join(process.cwd(), "assets", "og");
  const [regular, medium, demibold] = await Promise.all(["BDOGrotesk-Regular.ttf", "BDOGrotesk-Medium.ttf", "BDOGrotesk-DemiBold.ttf"].map(name => readFile(join(dir, name))));
  return [
    { name: "BDO Grotesk", data: regular, weight: 400 as const, style: "normal" as const },
    { name: "BDO Grotesk", data: medium, weight: 500 as const, style: "normal" as const },
    { name: "BDO Grotesk", data: demibold, weight: 600 as const, style: "normal" as const },
  ];
})();

const clip = (text: string, max: number) => (text.length <= max ? text : `${text.slice(0, max - 1).trimEnd()}…`);

export async function GET() {
  const headline = clip(appConfig.landing.headline.replace(/\s*\n\s*/g, " "), 64);
  return new ImageResponse(
    (
      <div style={{ width: "100%", height: "100%", display: "flex", flexDirection: "column", justifyContent: "space-between", padding: "84px 96px", backgroundColor: PAPER, color: INK, fontFamily: "BDO Grotesk" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 18, fontSize: 34, fontWeight: 500 }}>
          {/* eslint-disable-next-line @next/next/no-img-element -- next/og draws plain elements */}
          <img src={svgDataUri(markSvg())} width={56} height={56} alt="" style={{ borderRadius: 12 }} />
          <span>Team of Silicons&nbsp;<span style={{ color: MUTED }}>{appConfig.name}</span></span>
        </div>
        <div style={{ display: "flex", flexDirection: "column", gap: 28, maxWidth: 1000 }}>
          <div style={{ fontSize: 84, fontWeight: 600, lineHeight: 1.06, letterSpacing: -2.5, color: INK }}>{headline}</div>
          <div style={{ fontSize: 30, color: SECONDARY, lineHeight: 1.35 }}>{clip(appConfig.tagline, 110)}</div>
        </div>
        <div style={{ display: "flex" }} />
      </div>
    ),
    { width: 1200, height: 630, fonts: await fonts, headers: { "Cache-Control": "public, max-age=3600, stale-while-revalidate=86400" } },
  );
}
