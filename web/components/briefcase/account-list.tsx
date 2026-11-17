"use client";

/**
 * The account menu's list (foundation/shell/profile-menu.tsx) as a page section, for Organizations: every account signed
 * in to Briefcase on this browser, each photo, name and id line (a work account's names its organization), the current
 * one marked, the others one Switch away, and a last row to add another. Choosing an account is choosing which
 * organization you work in.
 */
import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Check, UserRoundPlus } from "lucide-react";
import { Avatar } from "@teamofsilicons/silicon-ui/avatar";
import { Badge } from "@teamofsilicons/silicon-ui/badge";
import { Button } from "@teamofsilicons/silicon-ui/button";
import { ActionItem, Row, Section, Tile } from "@/components/foundation/page/rows";
import { accountLine } from "@/components/foundation/shell/profile-menu";
import { SignInEnded, addAccount, switchAccount, useSignedInAccounts } from "@/lib/client/accounts";
import { notify, notifyError } from "@/lib/notify";

export function SignedInAccounts() {
  const { accounts } = useSignedInAccounts();
  const cache = useQueryClient();
  const [switching, setSwitching] = useState<string | null>(null);
  async function choose(uuid: string) {
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
  return (
    <Section id="your-accounts" title="Your accounts" count={accounts.length > 1 ? accounts.length : undefined} description="Each account has its own files. A work account works inside its organization.">
      {accounts.map(account => (
        <Row key={account.uuid} lead={<Avatar name={account.display_name} src={account.pfp_url ?? undefined} size="sm" aria-hidden="true" />} title={account.display_name} detail={accountLine(account)}>
          {account.current
            ? <Badge size="sm" tone="neutral" icon={<Check size={12} strokeWidth={2} />}>Current</Badge>
            : <Button size="sm" variant="secondary" loading={switching === account.uuid} disabled={!!switching} aria-label={`Switch to ${account.display_name}`} onClick={() => void choose(account.uuid)}>Switch</Button>}
        </Row>
      ))}
      <ActionItem onClick={addAccount} lead={<Tile><UserRoundPlus size={16} strokeWidth={2} /></Tile>} title="Add another account" detail="Sign in to a personal or work account in Silicon Accounts" />
    </Section>
  );
}
