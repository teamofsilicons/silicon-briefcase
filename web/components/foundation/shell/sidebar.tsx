"use client";

/**
 * Briefcase's section list, drawn as Silicon Accounts draws its own (silicon-accounts web/components/foundation/shell/
 * sidebar.tsx): Silicon UI's NavList with every section (lib/app.config.ts `nav`), a count where notifications wait, and
 * the other sites at the foot as small grey chips. Which account (personal or work) you are in is the account menu's
 * job (profile-menu.tsx), as on Accounts, so the list starts with the sections. The shell shows it beside the page from
 * 900px and inside a bottom sheet on narrower screens; both read this one list.
 */
import { usePathname } from "next/navigation";
import { useQuery } from "@tanstack/react-query";
import { Badge } from "@teamofsilicons/silicon-ui/badge";
import { ButtonLink } from "@teamofsilicons/silicon-ui/button-link";
import { NavItem, NavList } from "@teamofsilicons/silicon-ui/nav-list";
import { appConfig } from "@/lib/app.config";
import { api } from "@/lib/client/api";
import { navItemFor } from "@/lib/navigation";
import styles from "./sidebar.module.css";

export interface SidebarProps {
  /** Silicon Accounts' public origin, for the foot's Accounts chip. */
  accountsUrl: string;
  /** Called when a row is chosen (closes the phone sheet). */
  onNavigate?: () => void;
}

export function Sidebar({ accountsUrl, onNavigate }: SidebarProps) {
  const pathname = usePathname();
  const active = navItemFor(pathname);
  // Its own key, not the Notifications page's ["inbox"]: the page hydrates after the shell, and a shared cache filled in
  // between would make its first render differ from the server's.
  const inbox = useQuery({ queryKey: ["notifications-unread"], queryFn: () => api.get<{ unread_count: number }>("/v1/notifications"), refetchInterval: 60_000 });
  const unread = inbox.data?.unread_count ?? 0;
  const elsewhere = [
    { key: "docs", label: "Docs", href: appConfig.links.docs },
    { key: "store", label: "Apps", href: appConfig.links.store },
    { key: "accounts", label: "Accounts", href: accountsUrl },
  ];

  return (
    <div className={styles.sidebar}>
      <NavList label={`${appConfig.name} sections`} onNavigate={onNavigate}>
        {appConfig.nav.map(item => {
          const Icon = item.icon;
          const count = item.href === "/inbox" ? unread : 0;
          return (
            <NavItem
              key={item.href}
              href={item.href}
              icon={<Icon strokeWidth={2} aria-hidden="true" />}
              active={item.href === active?.href}
              trailing={count > 0 ? <Badge tone="warning" size="sm" aria-label={`${count} unread`}>{count}</Badge> : undefined}
            >
              {item.label}
            </NavItem>
          );
        })}
      </NavList>
      <nav className={styles.foot} aria-label="Other sites">
        {elsewhere.map(site => (
          <ButtonLink key={site.key} href={site.href} variant="soft" size="xs" target="_blank" rel="noopener" onClick={onNavigate}>
            {site.label}
            <span className="sr-only"> (opens in a new tab)</span>
          </ButtonLink>
        ))}
      </nav>
    </div>
  );
}
