"use client";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/client/api";
import { useSessionView } from "@/lib/client/session";
import type { Organizations } from "@/lib/briefcase/organizations";
/**
 * The current account's organizations. `enabled: false` only reads what is cached: the shell passes it until its menu
 * or palette opens, because the page hydrates after the shell, and a cache the shell filled in between would make the
 * page's first render differ from the server's.
 */
export function useOrganizations(enabled = true) {
  const session = useSessionView();
  return useQuery({ queryKey: ["organizations", session.account?.uuid], queryFn: () => api.get<Organizations>("/v1/organizations"), enabled: session.signedIn && enabled, refetchInterval: enabled ? 30_000 : false });
}
