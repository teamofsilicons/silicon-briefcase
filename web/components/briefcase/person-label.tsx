"use client";
import { useQuery } from "@tanstack/react-query";
import { AccountChip, type ChipAccount } from "@/components/foundation/account/account-chip";
import { useSessionView } from "@/lib/client/session";
import { api } from "@/lib/client/api";
import type { Person } from "@/lib/briefcase/people";
import { usePeople } from "./people-query";

/** Saved details are always matched by immutable UUID, never a reusable public ID. */
export function PersonLabel({ person, resolve = false }: { person: ChipAccount; resolve?: boolean }) {
  const people = usePeople();
  const session = useSessionView();
  const known = people.data?.items.find(p => p.uuid === person.uuid);
  const profile = useQuery({ queryKey: ["recipient", session.account?.uuid, person.uuid], queryFn: ({ signal }) => api.get<Person>("/v1/recipients/resolve", { query: { id: person.uuid }, signal }), enabled: resolve && !!person.uuid && !known, retry: false, staleTime: 30_000 });
  return <AccountChip account={known ?? profile.data ?? person} />;
}
