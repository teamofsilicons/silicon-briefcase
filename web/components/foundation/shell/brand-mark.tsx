/**
 * The top bar's brand, drawn as Silicon Accounts draws its own (silicon-accounts web/components/foundation/shell): the
 * app's glyph in ink, then "Team of Silicons Briefcase" in the body face at 16px, medium, with the product muted.
 * The glyph is Briefcase's logo mark (lib/app-logo.ts), in the text colour.
 */
import { appConfig } from "@/lib/app.config";
import { LOGO_MARK } from "@/lib/app-logo";
import styles from "./shell.module.css";

/** The mark alone, in the text colour, at the top bar's 18px. */
export function BrandMark() {
  return (
    <span className={styles.brandMark} aria-hidden="true">
      <svg viewBox={LOGO_MARK.viewBox} focusable="false" dangerouslySetInnerHTML={{ __html: LOGO_MARK.body }} />
    </span>
  );
}

/** The mark and "Team of Silicons Briefcase", as the top bar shows them. */
export function BrandWords() {
  return (
    <>
      <BrandMark />
      <span className={styles.brandText}>Team of Silicons <b className={styles.brandProduct}>{appConfig.name}</b></span>
    </>
  );
}
