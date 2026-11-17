"use client";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/client/api";
import { useSessionView } from "@/lib/client/session";
import type { Contacts, People } from "@/lib/briefcase/people";

export function usePeople(enabled = true) {
  const { account } = useSessionView();
  return useQuery({ queryKey: ["people", account?.uuid], queryFn: ({ signal }) => api.get<People>("/v1/recipients", { signal }), enabled: enabled && !!account, staleTime: 30_000 });
}
export function useContacts() {
  const { account } = useSessionView();
  return useQuery({ queryKey: ["contacts", account?.uuid], queryFn: ({ signal }) => api.get<Contacts>("/v1/contacts", { signal }), enabled: !!account, staleTime: 30_000 });
}
