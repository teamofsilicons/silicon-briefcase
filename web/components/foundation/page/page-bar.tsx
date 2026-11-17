"use client";

/**
 * The bar at the top of a workspace page, as on Silicon Accounts (its components/account/parts/page-bar.tsx): the way back on
 * nested pages (a page in the sidebar has none, so the bar is one line shorter), the page's title on the line under it, and
 * the page's supporting buttons on the title's right, pinned to the top of the page box with a line under it.
 * The current page is the title, so its crumb is hidden: Silicon UI's Breadcrumb always ends on the current page and wouldn't
 * link a lone parent. The title is the page's h1 role, as on every page.
 */
import type { ReactNode } from "react";
import { Breadcrumb } from "@teamofsilicons/silicon-ui/breadcrumb";
import styles from "./page-bar.module.css";

export interface Crumb {
  label: string;
  href?: string;
  onClick?: () => void;
}

/** `parent` is one way back; `trail` is every step above the page (a folder's folders), shortened in the middle when long. */
export function PageBar({ parent, trail, title, actions }: { parent?: { label: string; href: string }; trail?: Crumb[]; title: string; actions?: ReactNode }) {
  const crumbs = trail?.length ? trail : parent ? [parent] : [];
  return (
    <div className={styles.bar}>
      {crumbs.length ? <Breadcrumb items={[...crumbs, { label: title }]} maxItems={4} /> : null}
      <div className={styles.row}>
        <h1 className={styles.title}>{title}</h1>
        {actions}
      </div>
    </div>
  );
}
