"use client";

/**
 * The account menu at the top bar's end, copied from Silicon Accounts' profile menu (silicon-accounts web/components/
 * foundation/shell/profile-menu.tsx; decisions.md, 2026-11-18 "Browser profiles"): every account signed in to Briefcase on
 * this browser, each name above its id in an equal-height row with a check on the current one, then Add Another Account,
 * Your Accounts and Sign Out. A work account's id line names its organization, so choosing an account is choosing where
 * you work: this replaces the sidebar's workspace switcher. A dropdown from 640px; a bottom sheet on phones.
 */
import { useState, useSyncExternalStore } from "react";
import { Building2, Check, LogOut, UserRoundPlus, UsersRound } from "lucide-react";
import { Avatar } from "@teamofsilicons/silicon-ui/avatar";
import { Button } from "@teamofsilicons/silicon-ui/button";
import { Drawer, DrawerContent, DrawerTrigger } from "@teamofsilicons/silicon-ui/drawer";
import { DropdownMenuContent, DropdownMenuItem, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuRoot, DropdownMenuSeparator, DropdownMenuTrigger } from "@teamofsilicons/silicon-ui/dropdown-menu";
import { NavItem, NavList } from "@teamofsilicons/silicon-ui/nav-list";
import { useQueryClient } from "@tanstack/react-query";
import type { SessionAccount } from "@/lib/account";
import { SignInEnded, addAccount, switchAccount, useSignedInAccounts, type SignedInAccount } from "@/lib/client/accounts";
import { notify, notifyError } from "@/lib/notify";
import styles from "./profile-menu.module.css";

const phoneQuery = "(max-width: 639px)";
const subscribePhone = (change: () => void) => {
  const query = window.matchMedia(phoneQuery);
  query.addEventListener("change", change);
  return () => query.removeEventListener("change", change);
};

/** The id line: a work account's organization, then the id. */
export function accountLine(account: Pick<SignedInAccount, "id" | "org">) {
  return account.org ? `${account.org.name} · ${account.id}` : account.id;
}

export function ProfileMenu({ account, accountsUrl, onSignOut, signingOut }: {
  account: SessionAccount;
  /** Silicon Accounts' public origin: Your Accounts opens its list of your accounts. */
  accountsUrl: string;
  onSignOut: () => void | Promise<void>;
  signingOut: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [switching, setSwitching] = useState<string | null>(null);
  const phone = useSyncExternalStore(subscribePhone, () => window.matchMedia(phoneQuery).matches, () => false);
  const cache = useQueryClient();
  const { accounts } = useSignedInAccounts(open);
  const profiles: SignedInAccount[] = accounts.length ? accounts : [{ ...account, org: null, current: true }];

  async function selectProfile(uuid: string) {
    setOpen(false);
    if (uuid === account.uuid) return;
    setSwitching(uuid);
    try {
      await switchAccount(uuid);
    } catch (error) {
      setSwitching(null);
      if (error instanceof SignInEnded) {
        notify.error(error.message);
        void cache.invalidateQueries({ queryKey: ["signed-in-accounts"] });
      } else notifyError(error, "Could not switch accounts");
    }
  }

  const actions = [
    { id: "add", label: "Add another account", Icon: UserRoundPlus, select: () => { setOpen(false); addAccount(); } },
    { id: "manage", label: "Your accounts", Icon: UsersRound, select: () => { setOpen(false); window.open(`${accountsUrl}/linked-accounts`, "_blank", "noopener"); } },
    { id: "sign-out", label: signingOut ? "Signing out" : "Sign out", Icon: LogOut, select: () => {
      const done = onSignOut();
      if (done) void done.finally(() => setOpen(false));
      else setOpen(false);
    } },
  ];

  const trigger = (
    <Button variant="ghost" className={styles.trigger} aria-label={`Account menu for ${account.display_name}`} loading={!!switching}>
      <Avatar name={account.display_name} src={account.pfp_url ?? undefined} size="md" aria-hidden="true" />
    </Button>
  );
  const rows = profiles.map(profile => {
    const avatar = <Avatar name={profile.display_name} src={profile.pfp_url ?? undefined} size="md" aria-hidden="true" />;
    const label = (
      <span className={styles.identity}>
        <span className={styles.name} title={profile.display_name}>{profile.display_name}</span>
        <span className={styles.handle} title={accountLine(profile)}>
          {profile.org ? <Building2 className={styles.orgIcon} size={12} strokeWidth={2} aria-hidden="true" /> : null}
          {accountLine(profile)}
        </span>
      </span>
    );
    if (phone) {
      return <NavItem key={profile.uuid} active={profile.current} icon={avatar} trailing={profile.current && <Check size={16} aria-hidden="true" />} onSelect={() => void selectProfile(profile.uuid)}>{label}</NavItem>;
    }
    return <DropdownMenuRadioItem key={profile.uuid} value={profile.uuid} textValue={`${profile.display_name} ${accountLine(profile)}`} icon={avatar}>{label}</DropdownMenuRadioItem>;
  });
  const utilities = actions.map(({ id, label, Icon, select }) => {
    const icon = <span className={styles.actionIcon}><Icon size={18} strokeWidth={1.8} aria-hidden="true" /></span>;
    return phone
      ? <NavItem key={id} icon={icon} onSelect={select} disabled={signingOut}>{label}</NavItem>
      : <DropdownMenuItem key={id} icon={icon} disabled={signingOut} onSelect={event => { event.preventDefault(); select(); }}>{label}</DropdownMenuItem>;
  });

  if (phone) return (
    <Drawer open={open} onOpenChange={setOpen}>
      <DrawerTrigger asChild>{trigger}</DrawerTrigger>
      <DrawerContent side="bottom" title="Switch accounts">
        <NavList label="Signed-in accounts">{rows}</NavList>
        <div className={styles.sheetActions}><NavList label="Account actions">{utilities}</NavList></div>
      </DrawerContent>
    </Drawer>
  );
  return (
    <DropdownMenuRoot open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>{trigger}</DropdownMenuTrigger>
      <DropdownMenuContent className={styles.menu} align="end">
        <DropdownMenuRadioGroup aria-label="Signed-in accounts" value={account.uuid} onValueChange={uuid => void selectProfile(uuid)}>{rows}</DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        {utilities}
      </DropdownMenuContent>
    </DropdownMenuRoot>
  );
}
