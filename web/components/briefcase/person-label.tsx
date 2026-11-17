"use client";
import { useQuery } from "@tanstack/react-query";
import { Avatar } from "@teamofsilicons/silicon-ui/avatar";
import { AccountChip, type ChipAccount } from "@/components/foundation/account/account-chip";
import { useSessionView } from "@/lib/client/session";
import { api } from "@/lib/client/api";
import type { Person } from "@/lib/briefcase/people";
import { usePeople } from "./people-query";

/**
 * The best-known details for an account: a saved contact first, then (with `resolve`) its public profile, else what the
 * caller has. Saved details are always matched by immutable UUID, never a reusable public ID.
 */
export function usePerson(person: ChipAccount, resolve = false): ChipAccount {
  const people = usePeople();
  const session = useSessionView();
  const known = people.data?.items.find(p => p.uuid === person.uuid);
  const profile = useQuery({ queryKey: ["recipient", session.account?.uuid, person.uuid], queryFn: ({ signal }) => api.get<Person>("/v1/recipients/resolve", { query: { id: person.uuid }, signal }), enabled: resolve && !!person.uuid && !known, retry: false, staleTime: 30_000 });
  return known ?? profile.data ?? person;
}

export function PersonLabel({ person, resolve = false }: { person: ChipAccount; resolve?: boolean }) {
  return <AccountChip account={usePerson(person, resolve)} />;
}

/** A person's photo (or initials) at a row's lead size, 28px like the rows' tiles. */
export function PersonAvatar({ person }: { person: ChipAccount }) {
  const name = person.display_name?.trim() || person.id.replace(/^(c|si):/, "");
  return <Avatar name={name} src={person.pfp_url ?? person.fallback_pfp_url ?? undefined} size="sm" aria-hidden="true" />;
}
