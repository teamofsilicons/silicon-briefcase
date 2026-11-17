"use client";

/**
 * The accounts signed in to Briefcase on this browser, for the account menu and Organizations (as Silicon Accounts lists
 * its browser profiles): the current account, then the others from GET /auth/accounts, each with its work organization.
 * Switching goes through POST /auth/accounts/switch and then a full navigation, so nothing from the last account stays
 * in the page's caches. Adding one goes to Silicon Accounts' account chooser, where work accounts are listed with their
 * organizations.
 */
import { createContext, useContext } from "react";
import { useQuery } from "@tanstack/react-query";
import type { SessionAccount } from "@/lib/account";
import { useOrganizations } from "@/components/briefcase/organizations-query";
import { useSessionView } from "./session";

/**
 * The account the workspace layout signed in with, given by the shell (app-shell.tsx). The browser's session store only
 * knows the account after hydration, so a page that reads it there would render "signed out" first and then change;
 * this has the account from the first render, the server's included.
 */
const CurrentAccountContext = createContext<SessionAccount | null>(null);
export const CurrentAccountProvider = CurrentAccountContext.Provider;
export function useCurrentAccount(): SessionAccount | null {
  const fromLayout = useContext(CurrentAccountContext);
  return useSessionView().account ?? fromLayout;
}

export interface SignedInAccount {
  uuid: string;
  kind: SessionAccount["kind"];
  id: string;
  display_name: string;
  pfp_url: string | null;
  /** A work account's organization; null for a personal account (or not known yet). */
  org: { name: string; logo_url: string | null } | null;
  current: boolean;
}

/** Silicon Accounts' account chooser, back to My files: it lists the person's personal and work accounts. */
export const ADD_ACCOUNT = "/auth/sign-in?prompt=select_account&return_to=%2Ffiles";

/** Where switching lands: the new account's own files. */
const AFTER_SWITCH = "/files";

/** A work account's id carries its organization (`c:ada.example`); a personal one has none. */
export const isWorkAccount = (account: { id: string } | null | undefined) => !!account?.id.split(":")[1]?.includes(".");

/**
 * `enabled: false` reads only what is cached (see useOrganizations): the shell's menu and palette fetch when they open,
 * never while a page hydrates.
 */
export function useSignedInAccounts(enabled = true) {
  const account = useCurrentAccount();
  const organizations = useOrganizations(enabled);
  const others = useQuery({
    queryKey: ["signed-in-accounts", account?.uuid],
    enabled: !!account && enabled,
    staleTime: 30_000,
    queryFn: async ({ signal }) => {
      const response = await fetch("/auth/accounts", { credentials: "same-origin", cache: "no-store", headers: { Accept: "application/json" }, signal });
      if (!response.ok) throw new Error("Your other accounts could not be read.");
      return ((await response.json()) as { accounts: Omit<SignedInAccount, "current">[] }).accounts;
    },
  });
  const org = organizations.data?.organizations[0];
  const current: SignedInAccount | null = account
    ? { uuid: account.uuid, kind: account.kind, id: account.id, display_name: account.display_name, pfp_url: account.pfp_url, org: org ? { name: org.name, logo_url: org.logo_url } : null, current: true }
    : null;
  return {
    accounts: [...(current ? [current] : []), ...(others.data ?? []).map(account => ({ ...account, current: false }))],
    pending: others.isPending,
    error: others.error,
  };
}

/** Thrown when an account's sign-in has ended; the account has left the list. */
export class SignInEnded extends Error {}

/** Makes another signed-in account current, then opens its files. */
export async function switchAccount(uuid: string): Promise<void> {
  const response = await fetch("/auth/accounts/switch", {
    method: "POST",
    credentials: "same-origin",
    cache: "no-store",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
    body: JSON.stringify({ account_uuid: uuid }),
  });
  if (response.ok) {
    // A full navigation, not a client one: the last account's queries and page state must not survive the switch.
    // eslint-disable-next-line @next/next/no-location-assign-relative-destination
    window.location.assign(AFTER_SWITCH);
    return;
  }
  const body = (await response.json().catch(() => null)) as { error?: { message?: string; hint?: string } } | null;
  const words = [body?.error?.message, body?.error?.hint].filter(Boolean).join(" ") || "That account could not be opened. Please try again.";
  throw response.status === 409 ? new SignInEnded(words) : new Error(words);
}

/** Adds another account through Silicon Accounts' account chooser. */
export function addAccount(): void {
  // /auth/sign-in is a route handler that starts Silicon Accounts' account chooser, not a page.
  // eslint-disable-next-line @next/next/no-location-assign-relative-destination
  window.location.assign(ADD_ACCOUNT);
}
