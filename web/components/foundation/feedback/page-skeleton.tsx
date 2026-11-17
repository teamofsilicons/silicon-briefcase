/**
 * The loading stand-in for a workspace page, drawn like the page it becomes (Silicon Accounts' PageSkeleton): the page
 * bar pinned at the top, then sections on the page column, each a heading over a boxed list, so nothing jumps when the
 * page arrives. Announced once as "Loading" (aria-busy), never as a list of shapes.
 */
import { Page } from "@/components/foundation/layout/layout";
import { SkeletonBlock } from "./skeleton-block";
import styles from "./page-skeleton.module.css";

export function PageSkeleton() {
  return (
    <div aria-busy="true" aria-label="Loading">
      <div className={styles.bar}>
        <div className={styles.titleRow}><SkeletonBlock width="min(14rem, 60%)" height="1.75rem" radius="var(--radius-control)" /></div>
      </div>
      <Page>
        {[3, 2].map((rows, index) => (
          <div key={index} className={styles.section}>
            <SkeletonBlock width="8rem" height="1.25rem" radius="var(--radius-control)" index={index} />
            <SkeletonBlock width="100%" height={`calc(${rows} * 3.25rem)`} radius="var(--radius-container)" index={index} />
          </div>
        ))}
      </Page>
    </div>
  );
}
