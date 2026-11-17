/**
 * The loading stand-in for a workspace page, drawn like the page it becomes (Silicon Accounts' PageSkeleton): the page
 * bar pinned at the top, then sections on the page column, each a heading over a boxed list of row-shaped placeholders
 * (foundation/page/rows SectionSkeleton), so nothing jumps when the page arrives. Announced once as "Loading".
 */
import { Page } from "@/components/foundation/layout/layout";
import { SectionSkeleton } from "@/components/foundation/page/rows";
import { SkeletonBlock } from "./skeleton-block";
import styles from "./page-skeleton.module.css";

export function PageSkeleton() {
  return (
    <div aria-busy="true" aria-label="Loading">
      <div className={styles.bar}>
        <div className={styles.titleRow}><SkeletonBlock width="min(14rem, 60%)" height="1.5rem" radius="var(--radius-control)" /></div>
      </div>
      <Page>
        <SkeletonBlock width="min(28rem, 80%)" height="0.875rem" />
        <SectionSkeleton rows={4} label="Loading this page" />
        <SectionSkeleton rows={2} title label="Loading this page" />
      </Page>
    </div>
  );
}
