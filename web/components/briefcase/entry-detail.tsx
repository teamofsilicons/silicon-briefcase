"use client";
import { useState } from "react";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, ExternalLink, Folder, RotateCcw } from "lucide-react";
import { Button } from "@/components/arc/button/button";
import { Input } from "@/components/arc/input/input";
import { Select } from "@/components/arc/select/select";
import { Dialog, DialogContent } from "@/components/arc/dialog/dialog";
import { HoldToConfirm } from "@/components/arc/hold-to-confirm/hold-to-confirm";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { api } from "@/lib/client/api";
import { notify } from "@/lib/notify";
import { contentUrl, date, entryUrl, message, size, type Activity, type Entry, type EntryPage, type Grant, type LinkAccess, type Version } from "@/lib/briefcase/types";
import { Preview } from "./preview";
import styles from "./briefcase.module.css";

export function EntryDetail({ entry: initial, onClose, onChanged }: { entry: Entry; onClose: () => void; onChanged: () => void }) {
  const cache = useQueryClient();
  const [tab, setTab] = useState("preview");
  const [name, setName] = useState(initial.name);
  const [destination, setDestination] = useState<string | null>(null);
  const [recipient, setRecipient] = useState("");
  const [rights, setRights] = useState("read");
  const [minutes, setMinutes] = useState("");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const [selectedFolder, setSelectedFolder] = useState("");
  const [moving, setMoving] = useState(false);
  const endpoint = entryUrl(initial.id);
  const entryQuery = useQuery({ queryKey: ["entry", initial.id], queryFn: () => api.get<Entry>(endpoint), initialData: initial, enabled: !initial.deleted_at });
  const entry = entryQuery.data;
  const link = useQuery({ queryKey: ["link", entry.id], queryFn: () => api.get<LinkAccess>(`${endpoint}/link-access`), enabled: tab === "sharing" && !entry.deleted_at });
  const invitations = useInfiniteQuery({ initialPageParam: null as string | null, getNextPageParam: (last: { next_cursor: string | null }) => last.next_cursor ?? undefined, queryKey: ["grants", entry.id], queryFn: ({ pageParam }) => api.get<{ items: Grant[]; next_cursor: string | null }>(`${endpoint}/invitations`, { query: { cursor: pageParam } }), enabled: tab === "sharing" && !!link.data?.can_manage && !entry.deleted_at });
  const versions = useInfiniteQuery({ initialPageParam: null as string | null, getNextPageParam: (last: { next_cursor: string | null }) => last.next_cursor ?? undefined, queryKey: ["versions", entry.id], queryFn: ({ pageParam }) => api.get<{ items: Version[]; next_cursor: string | null }>(`${endpoint}/versions`, { query: { cursor: pageParam } }), enabled: tab === "versions" && entry.type === "file" && !entry.deleted_at });
  const activity = useQuery({ queryKey: ["activity", entry.id], queryFn: () => api.get<{ items: Activity[] }>(`${endpoint}/activity`), enabled: tab === "activity" && !entry.deleted_at });
  const folderQuery = useInfiniteQuery({ initialPageParam: null as string | null, getNextPageParam: (last: EntryPage) => last.next_cursor ?? undefined, queryKey: ["move-folders", entry.org_id, selectedFolder], queryFn: ({ pageParam }) => api.get<EntryPage>("/v1/entries", { query: { drive: entry.org_id, path: selectedFolder, cursor: pageParam } }), enabled: moving });
  async function act(work: () => Promise<unknown>, success: string, close = false) {
    setBusy(true); setFailure("");
    try { await work(); notify.success(success); await cache.invalidateQueries({ queryKey: ["entry", entry.id] }); await cache.invalidateQueries({ queryKey: ["grants", entry.id] }); await cache.invalidateQueries({ queryKey: ["link", entry.id] }); await cache.invalidateQueries({ queryKey: ["versions", entry.id] }); onChanged(); if (close) onClose(); }
    catch (error) { setFailure(message(error)); } finally { setBusy(false); }
  }
  const canUpdate = entry.effective_access.includes("update");
  const canDelete = entry.effective_access.includes("delete");
  async function move() {
    const parent = destination ?? (await api.get<Entry>(`/v1/paths/${encodeURIComponent(entry.org_id)}${selectedFolder ? `/${selectedFolder.split("/").map(encodeURIComponent).join("/")}` : ""}`)).id;
    await api.patch(endpoint, { parent_id: parent }, { idempotencyKey: true }); setMoving(false);
  }
  return <Dialog open onOpenChange={open => { if (!open) onClose(); }}><DialogContent className={styles.dialog} title={entry.name} description={entry.type === "folder" ? "Folder details and sharing" : `${size(entry.size)} · ${entry.content_type || "File"}`}>
    {failure && <p role="alert" data-sq="surface" className={styles.error}>{failure}</p>}
    {entry.deleted_at ? <div className={styles.form}><p>This entry was moved to the bin on {date(entry.deleted_at)}. Restore it before its 45-day retention ends.</p><Button loading={busy} onClick={() => void act(() => api.post(`/v1/bin/${entry.id}/restore`, {}), "Entry restored", true)}><RotateCcw size={16} />Restore from bin</Button></div> : <>
      <div className={styles.tabs} role="tablist" aria-label="File details">{["preview", "sharing", ...(entry.type === "file" ? ["versions"] : []), "activity", "details"].map(value => <Button key={value} role="tab" aria-selected={tab === value} data-active={tab === value} size="sm" variant="ghost" onClick={() => setTab(value)}>{value[0].toUpperCase() + value.slice(1)}</Button>)}</div>
      {tab === "preview" && <><Preview entry={entry} /><div className={styles.actions}><a href={contentUrl(entry.id, true)} className={styles.rowButton}><Download size={17} />Download {entry.type === "folder" ? "folder" : "file"}</a><a href={new URL(entry.permanent_url).pathname} className={styles.rowButton}><ExternalLink size={17} />Open permanent address</a></div></>}
      {tab === "sharing" && <div className={styles.form}>
        {link.error && <ErrorAlert error={link.error} />}{invitations.error && <ErrorAlert error={invitations.error} />}
        {link.data?.can_manage && <><div><h3>Share with an account</h3><p className={styles.muted}>Name a Carbon or Silicon by its current id. Delete access stays with the owner.</p></div>
        <form className={styles.form} onSubmit={event => { event.preventDefault(); if (!/^(c|si):[^\s]+$/.test(recipient.trim())) { setFailure("Enter a Carbon or Silicon id, such as c:ada or si:scout."); return; } void act(() => api.post(`${endpoint}/invitations`, { principal: { type: recipient.startsWith("si:") ? "silicon" : "carbon", id: recipient.trim() }, access: minutes ? ["read"] : rights.split(","), inherit: entry.type === "folder", ...(minutes ? { expires_in_minutes: Number(minutes) } : {}) }), "Access shared"); }}>
          <Input label="Account id" value={recipient} onChange={event => setRecipient(event.target.value)} placeholder="c:ada or si:scout" required />
          <div className={styles.detailGrid}><Select label="Access" value={rights} onValueChange={setRights} disabled={!!minutes} options={[{value:"read",label:"Read"},{value:"read,write",label:"Read and add"},{value:"read,update",label:"Read and edit"},{value:"read,write,update",label:"Read, add and edit"}]} /><Input label="Expires after (minutes)" type="number" min={1} max={43200} placeholder="No expiry" value={minutes} onChange={event => setMinutes(event.target.value)} /></div>
          {minutes && <p className={styles.muted}>Expiring access is read-only and lasts from one minute to thirty days.</p>}<Button type="submit" loading={busy}>Share access</Button>
        </form>
        <ul className={styles.list}>{invitations.data?.pages.flatMap(page => page.items).map(grant => <li key={grant.id}><div><strong>{grant.principal.public_id || grant.principal.id}</strong><small>{grant.access.join(", ")} · {grant.expires_at ? `Until ${date(grant.expires_at)}` : "No expiry"}</small></div><Button variant="ghost" size="sm" disabled={busy} onClick={() => void act(() => api.delete(`${endpoint}/invitations/${grant.id}`), "Access removed")}>Remove</Button>{grant.expires_at && <Button variant="ghost" size="sm" onClick={() => void act(() => api.patch(`${endpoint}/invitations/${grant.id}`, minutes ? { expires_in_minutes: Number(minutes) } : { permanent: true }), minutes ? "Expiry changed" : "Share made permanent")}>{minutes ? "Set new expiry" : "Keep sharing"}</Button>}</li>)}</ul>
        {invitations.hasNextPage && <Button loading={invitations.isFetchingNextPage} onClick={() => void invitations.fetchNextPage()}>More accounts</Button>}</>}
        <div><h3>Anyone with the link</h3><p className={styles.muted}>{link.data?.effective ? "This entry can be opened without signing in." : "Only accounts with access can open this entry."}</p>{link.data?.url && <Input label="Shareable address" readOnly value={link.data.url} onFocus={event => event.target.select()} />}</div>
        {link.data?.can_manage && <div className={styles.actions}><Button variant="secondary" loading={busy} onClick={() => void act(() => api.put(`${endpoint}/link-access`, { enabled: !link.data?.enabled, ...(!link.data?.enabled && minutes ? { expires_in_minutes: Number(minutes) } : {}) }), link.data?.enabled ? "Link access turned off" : "Link access enabled")}>{link.data.enabled ? "Turn off link access" : "Enable link access"}</Button>{link.data.url && <Button variant="ghost" onClick={() => void navigator.clipboard.writeText(link.data!.url!).then(() => notify.success("Link copied"))}>Copy link</Button>}</div>}
      </div>}
      {tab === "versions" && <>{versions.error && <ErrorAlert error={versions.error} />}<ul className={styles.list}>{versions.data?.pages.flatMap(page => page.items).map(version => <li key={version.id}><div><strong>{date(version.created_at)}</strong><small>{size(version.size)}{version.created_by?.id && ` · ${version.created_by.id}`}</small></div>{canUpdate && <Button variant="secondary" size="sm" loading={busy} onClick={() => void act(() => api.post(`${endpoint}/versions/${version.id}/restore`, {}), "Version restored")}>Restore version</Button>}</li>)}</ul>{versions.data?.pages[0]?.items.length === 0 && <p className={styles.muted}>No earlier versions are available.</p>}{versions.hasNextPage && <Button loading={versions.isFetchingNextPage} onClick={() => void versions.fetchNextPage()}>More versions</Button>}</>}
      {tab === "activity" && <>{activity.error && <ErrorAlert error={activity.error} />}<ul className={styles.list}>{activity.data?.items.map((event, index) => <li key={event.id || index}><div><strong>{(event.action || event.event || event.kind || "File updated").replaceAll("_", " ")}</strong><small>{event.actor?.id} · {date(event.occurred_at)}</small></div></li>)}</ul>{activity.data?.items.length === 0 && <p className={styles.muted}>No activity to show.</p>}</>}
      {tab === "details" && <div className={styles.form}>
        <dl className={styles.details}><dt>Location</dt><dd>{entry.path || "/"}</dd><dt>Owner</dt><dd>{entry.owner?.id || "—"}</dd><dt>Created</dt><dd>{date(entry.created_at)}</dd><dt>Updated</dt><dd>{date(entry.updated_at)}</dd><dt>Your access</dt><dd>{entry.effective_access.join(", ")}</dd>{entry.origin_app_id && <><dt>Created by app</dt><dd>{entry.origin_app_id}</dd></>}</dl>
        {canUpdate && <form className={styles.form} onSubmit={event => { event.preventDefault(); void act(() => api.patch(endpoint, { name: name.trim() }, { idempotencyKey: true }), "Name updated"); }}><Input label="Name" value={name} onChange={event => setName(event.target.value)} required /><div className={styles.actions}><Button type="submit" loading={busy}>Rename</Button><Button type="button" variant="secondary" onClick={() => setMoving(!moving)}>Move to folder</Button></div></form>}
        {moving && <div className={styles.form}><p className={styles.muted}>Choose a folder in this drive.</p><Button size="sm" variant="ghost" onClick={() => { setSelectedFolder(""); setDestination(null); }}>Drive root</Button>{selectedFolder && <p>{selectedFolder}</p>}{folderQuery.error && <ErrorAlert error={folderQuery.error} />}<ul className={styles.list}>{folderQuery.data?.pages.flatMap(page => page.items).filter(item => item.type === "folder" && item.id !== entry.id).map(item => <li key={item.id}><Button variant="ghost" onClick={() => { setSelectedFolder(item.path); setDestination(item.id); }}><Folder size={16} />{item.name}</Button></li>)}</ul>{folderQuery.hasNextPage && <Button loading={folderQuery.isFetchingNextPage} onClick={() => void folderQuery.fetchNextPage()}>More folders</Button>}<Button loading={busy} onClick={() => void act(move, "Entry moved")}>Move here</Button></div>}
        {entry.self_destruct_at && <div><p data-sq="surface" className={styles.note}>This file deletes permanently on {date(entry.self_destruct_at)}. Removing it early also skips the bin.</p><Button variant="secondary" loading={busy} onClick={() => void act(() => api.delete(`${endpoint}/self-destruct`), "File kept permanently")}>Keep this file</Button></div>}
        {canDelete && <HoldToConfirm label={entry.self_destruct_at ? "Hold to delete permanently" : "Hold to move to bin"} tone="danger" disabled={busy} onConfirm={() => void act(() => api.delete(endpoint), entry.self_destruct_at ? "File deleted permanently" : "Moved to bin", true)} />}
      </div>}
    </>}
  </DialogContent></Dialog>;
}
