"use client";

/**
 * The workspace pages' sections and rows, as on Silicon Accounts (its components/account/parts/rows.tsx): an h2 over one
 * white list with a clear border, rows 8px high and 12px at the sides with straight
 * dividers, a lead (a 28px tile or mark), the title in body over one detail line in --text-soft, and the row's buttons at
 * its end. A LinkRow is a whole row that goes somewhere, with a chevron.
 */
import type { ReactNode } from "react";
import { ChevronRight } from "lucide-react";
import Link from "next/link";
import { Badge } from "@teamofsilicons/silicon-ui/badge";
import styles from "./rows.module.css";

/**
 * A section: its h2 (and a count, and a line under it) over a boxed list. Without a title the list stands alone (the page
 * bar names it). `tone="danger"` draws the box's line in red, the prototype's danger zone (.dz), for deleting.
 */
export function Section({ id, title, count, label, description, tone, children }: { id: string; title?: string; count?: number; label?: string; description?: ReactNode; tone?: "danger"; children: ReactNode }) {
  return (
    <section className={styles.section} aria-labelledby={title ? id : undefined} aria-label={title ? undefined : label}>
      {title ? (
        <div className={styles.headBlock}>
          <div className={styles.head}>
            <h2 id={id} className={styles.title}>{title}</h2>
            {count ? <Badge size="sm" tone="neutral">{count}</Badge> : null}
          </div>
          {description ? <p className={styles.description}>{description}</p> : null}
        </div>
      ) : null}
      <ul data-sq="surface" className={styles.rows} data-tone={tone}>{children}</ul>
    </section>
  );
}

/** One row. `wide` lets two buttons go under the words on a phone; a single button stays at the end. */
export function Row({ lead, title, detail, error, wide, children }: { lead: ReactNode; title: ReactNode; detail?: ReactNode; error?: string | null; wide?: boolean; children?: ReactNode }) {
  return (
    <li className={styles.row} data-wide={wide ? "" : undefined}>
      {lead}
      <span className={styles.text}>
        <span className={styles.rowTitle}>{title}</span>
        {detail ? <span className={styles.rowDetail}>{detail}</span> : null}
        {error ? <span className={styles.rowError} role="alert">{error}</span> : null}
      </span>
      {children ? <span className={styles.actions}>{children}</span> : null}
    </li>
  );
}

/** A row inside a Section that is a link: the whole row goes to `href`, a chevron at its end; `end` sits before it. */
export function LinkItem({ href, lead, title, detail, end, label, replace, scroll }: { href: string; lead: ReactNode; title: ReactNode; detail?: ReactNode; end?: ReactNode; label?: string; replace?: boolean; scroll?: boolean }) {
  return (
    <li className={styles.linkItem}>
      <Link href={href} replace={replace} scroll={scroll} className={styles.link} aria-label={label}>
        {lead}
        <span className={styles.text}>
          <span className={styles.rowTitle}>{title}</span>
          {detail ? <span className={styles.rowDetail}>{detail}</span> : null}
        </span>
        {end ? <span className={styles.end}>{end}</span> : null}
        <ChevronRight className={styles.chevron} size={16} strokeWidth={2} aria-hidden="true" />
      </Link>
    </li>
  );
}

/**
 * A row inside a Section that opens something in place (a file's details, a folder): its lead and words are one button,
 * and the row's own buttons (`children`) sit after it, outside the button.
 */
export function ActionItem({ onClick, lead, title, detail, end, label, children }: { onClick: () => void; lead: ReactNode; title: ReactNode; detail?: ReactNode; end?: ReactNode; label?: string; children?: ReactNode }) {
  return (
    <li className={styles.actionItem}>
      <button type="button" className={styles.link} onClick={onClick} aria-label={label}>
        {lead}
        <span className={styles.text}>
          <span className={styles.rowTitle}>{title}</span>
          {detail ? <span className={styles.rowDetail}>{detail}</span> : null}
        </span>
        {end ? <span className={styles.end}>{end}</span> : null}
      </button>
      {children ? <span className={styles.itemActions}>{children}</span> : null}
    </li>
  );
}

/** A row on its own (no section) that is a link, a chevron at its end: Security's Activity, Connected apps' permissions. */
export function LinkRow({ href, lead, title, detail, end }: { href: string; lead: ReactNode; title: ReactNode; detail?: ReactNode; end?: ReactNode }) {
  return (
    <Link href={href} data-sq="surface" className={styles.linkRow}>
      {lead}
      <span className={styles.text}>
        <span className={styles.rowTitle}>{title}</span>
        {detail ? <span className={styles.rowDetail}>{detail}</span> : null}
      </span>
      {end ? <span className={styles.end}>{end}</span> : null}
      <ChevronRight className={styles.chevron} size={16} strokeWidth={2} aria-hidden="true" />
    </Link>
  );
}

/** A 28px grey tile holding an icon, the lead of a row that has no mark of its own. */
export const Tile = ({ children }: { children: ReactNode }) => <span data-sq="surface" className={styles.icon} aria-hidden="true">{children}</span>;

/** The information in a detail line ("3 weeks ago"), in the text colour, as on Home's app cards. */
export const Strong = ({ children }: { children: ReactNode }) => <span className={styles.strong}>{children}</span>;
