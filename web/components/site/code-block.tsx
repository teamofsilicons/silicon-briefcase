/**
 * A code block on the public pages, as Silicon Accounts has it (its components/site/code-block.tsx): Arc's
 * code-block anatomy (a squircle panel, a header with its title and a copy button, the source in the monospace face),
 * highlighted on the server (lib/highlight.ts) so the page ships plain spans. The copy button is Silicon UI's
 * CopyButton, the block's one client island.
 */
import { CopyButton } from "@teamofsilicons/silicon-ui/copy-button";
import { highlight, languageLabel } from "@/lib/highlight";
import styles from "./code-block.module.css";

/** `wrap` folds long lines (an installer) instead of scrolling them sideways. */
export function CodeBlock({ code, lang = "sh", title, wrap }: { code: string; lang?: string; title?: string; wrap?: boolean }) {
  const tokens = highlight(code, lang);
  const label = languageLabel(lang);
  return (
    <figure className={wrap ? `${styles.block} ${styles.wrap}` : styles.block} data-sq="clip" data-docs-code="">
      <figcaption className={styles.header}>
        <span className={styles.label}>
          {title ? <span className={styles.title}>{title}</span> : null}
          <span className={title ? styles.language : styles.languageOnly}>{label}</span>
        </span>
        <CopyButton value={code} iconOnly variant="plain" className={styles.copy} label={title ? `Copy ${title}` : `Copy ${label} code`} />
      </figcaption>
      <pre className={styles.pre} tabIndex={0} aria-label={title ?? `${label} code`} data-lang={lang || undefined}>
        <code>
          {tokens.map((token, index) => (token.k ? <span key={index} className={styles[token.k]}>{token.v}</span> : token.v))}
        </code>
      </pre>
    </figure>
  );
}
