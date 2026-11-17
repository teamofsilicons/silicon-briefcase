/**
 * The frame for pages anyone can open (a shared link): the workspace's top bar with the brand and, at its end, the way in
 * (Sign In), then the white page box, with no section list. The same look as the signed-in shell, as Silicon Accounts'
 * hosted pages share its frame. Server-safe.
 */
import Link from "next/link";
import type { ReactNode } from "react";
import { SkipLink } from "@/components/foundation/layout/layout";
import { appConfig } from "@/lib/app.config";
import { BrandWords } from "./brand-mark";
import styles from "./shell.module.css";

export function PublicFrame({ end, children }: { end?: ReactNode; children: ReactNode }) {
  return (
    <div className={styles.shell}>
      <SkipLink href="#main" />
      <header className={styles.top}>
        <Link href="/" aria-label={`Team of Silicons ${appConfig.name}, home`} data-sq="surface" className={styles.brand}>
          <BrandWords />
        </Link>
        <span />
        <span className={styles.topEnd}>{end}</span>
      </header>
      <div className={styles.body} data-bare="">
        <main id="main" data-sq-native="" className={styles.main} tabIndex={-1}>
          <div className={styles.page}>{children}</div>
        </main>
      </div>
    </div>
  );
}
