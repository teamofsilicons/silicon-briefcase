import { ApiError } from "@/lib/errors";
export type Account = { uuid: string; id: string | null; kind: "carbon" | "silicon"; display_name?: string | null };
export type Drive = { owner: Account; relation: "own" | "custodial"; root_id: string; storage_external_id?: string; permanent_url: string };
export type Entry = {
  id: string; org_id: string; name: string; path: string; parent_id: string | null; type: "file" | "folder";
  size: number | null; content_type: string | null; render: string | null; visibility: string;
  updated_at: string | null; created_at: string | null; deleted_at: string | null; self_destruct_at: string | null;
  owner: { type: string; id: string } | null; origin_app_id: string | null; effective_access: string[];
  permanent_url: string;
};
export type EntryPage = { items: Entry[]; next_cursor: string | null };
export type SharedPage = { items: { entry: Entry; owner: Account }[]; next_cursor: string | null };
export type Grant = { id: string; principal: { type: string; id: string; public_id?: string }; access: string[]; inherit: boolean; expires_at: string | null };
export type LinkAccess = { enabled: boolean; effective: boolean; can_manage: boolean; inherited_from: string | null; expires_at: string | null; url: string | null };
export type Version = { id: string; size: number; content_type: string; created_at: string; created_by?: { id: string } };
export type Activity = { id: string; action?: string; event?: string; kind?: string; actor?: { id: string }; occurred_at: string };
export type Usage = { storage: { used_bytes: number; limit_bytes: number }; daily_uploads: { used_bytes: number; limit_bytes: number; resets_at: string } };
export type Notification = { id: string; kind: string; read: boolean; actor: { id: string } | null; subject: { entry_id: string; name: string; permanent_url: string } | null; created_at: string };
export const size = (bytes: number | null | undefined) => {
  if (bytes == null) return "—";
  const n = bytes ? Math.min(5, Math.floor(Math.log(bytes) / Math.log(1024))) : 0;
  return `${(bytes / 1024 ** n).toLocaleString(undefined, { maximumFractionDigits: n ? 1 : 0 })} ${["B", "KiB", "MiB", "GiB", "TiB", "PiB"][n]}`;
};
export const date = (value: string | null | undefined) => value ? new Date(value).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : "—";
export const entryUrl = (id: string, suffix = "") => `/v1/entries/${encodeURIComponent(id)}${suffix}`;
export const contentUrl = (id: string, download = false) => `/api${entryUrl(id, download ? "/download" : "/content")}`;
export const message = (error: unknown) => error instanceof ApiError ? [error.message, error.hint].filter(Boolean).join(" ") : error instanceof Error ? error.message : "The request could not be completed.";
