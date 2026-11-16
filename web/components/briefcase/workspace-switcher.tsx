"use client";
import Link from "next/link";
import { Building2, ArrowLeftRight, UserRound } from "lucide-react";
import { useSessionView } from "@/lib/client/session";
import { useOrganizations } from "./organizations-query";
import styles from "./organizations.module.css";

export function WorkspaceSwitcher() {
  const query = useOrganizations();
  const organization = query.data?.organizations[0];
  const session = useSessionView();
  const workAccount = !!organization || !!session.account?.id.includes(".");
  return <div className={styles.switcher}>
    <Link href={organization ? `/organizations/${encodeURIComponent(organization.org_id)}` : workAccount ? "/organizations" : "/files"} className={styles.workspaceName}>
      {organization ? <Building2 size={19} aria-hidden="true" /> : <UserRound size={19} aria-hidden="true" />}
      <span><small>{query.isPending ? "Opening workspace" : query.error ? "Workspace unavailable" : workAccount ? "Work account" : "Personal account"}</small><strong>{organization?.name ?? (query.error ? "Try again in Organizations" : workAccount ? "Work workspace" : "My workspace")}</strong></span>
    </Link>
    {/* A full navigation starts Accounts' account chooser and clears old queries. */}
    {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
    <a href="/auth/sign-in?prompt=select_account&return_to=%2Ffiles" className={styles.switchAccount} aria-label="Switch personal or work account" title="Switch personal or work account"><ArrowLeftRight size={16} aria-hidden="true" /></a>
  </div>;
}
