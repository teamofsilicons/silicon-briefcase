"use client";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/client/api";
import { useSessionView } from "@/lib/client/session";
import type { Organizations } from "@/lib/briefcase/organizations";
export function useOrganizations() {
  const session = useSessionView();
  return useQuery({ queryKey: ["organizations", session.account?.uuid], queryFn: () => api.get<Organizations>("/v1/organizations"), enabled: session.signedIn, refetchInterval: 30_000 });
}
