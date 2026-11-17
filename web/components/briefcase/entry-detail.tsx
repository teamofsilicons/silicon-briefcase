"use client";

/**
 * A file's or folder's details, in Silicon UI's Dialog with Accounts' tabs: Preview, Sharing, Versions (files), Activity
 * and Details. The dialog keeps one size and place while it is open (pinned near the top, a fixed height): the tabs
 * never move under the pointer when a tab is shorter or its rows arrive late, and each tab scrolls inside itself. Each
 * tab is the page rules in small: sections (an h2 over one boxed list) of rows with a 28px lead, the title over one
 * detail line, the row's buttons at its end; an irreversible action held to confirm.
 */
import { useState } from "react";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { Building2, Download, ExternalLink, Folder, Globe, History, RotateCcw, Trash2, UsersRound } from "lucide-react";
import { Alert } from "@teamofsilicons/silicon-ui/alert";
import { Badge } from "@teamofsilicons/silicon-ui/badge";
import { Button } from "@teamofsilicons/silicon-ui/button";
import { ButtonLink } from "@teamofsilicons/silicon-ui/button-link";
import { CopyButton } from "@teamofsilicons/silicon-ui/copy-button";
import { Input } from "@teamofsilicons/silicon-ui/input";
import { Dialog, DialogContent } from "@teamofsilicons/silicon-ui/dialog";
import { HoldToConfirm } from "@teamofsilicons/silicon-ui/hold-to-confirm";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@teamofsilicons/silicon-ui/tabs";
import { DescriptionItem, DescriptionList } from "@/components/foundation/layout/layout";
import { ActionItem, Row, Section, SectionSkeleton, Tile } from "@/components/foundation/page/rows";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { api } from "@/lib/client/api";
import { notify } from "@/lib/notify";
import { contentUrl, date, entryUrl, message, size, type Activity, type Entry, type EntryPage, type Grant, type LinkAccess, type Version } from "@/lib/briefcase/types";
import { Preview } from "./preview";
import { PersonAvatar, usePerson } from "./person-label";
import { ShareAccess } from "./share-access";
import { kindOf } from "./file-kind";
import styles from "./entry-detail.module.css";

const TABS = ["preview", "sharing", "versions", "activity", "details"] as const;
const LABELS: Record<(typeof TABS)[number], string> = { preview: "Preview", sharing: "Sharing", versions: "Versions", activity: "Activity", details: "Details" };

/** What a share lets someone do, in the share form's own words. */
function accessLabel(access: string[]) {
  if (access.includes("write") && access.includes("update")) return "Can view, add, and edit";
  if (access.includes("update")) return "Can view and edit";
  if (access.includes("write")) return "Can view and add files";
  return "Can view and download";
}

type Act = (work: () => Promise<unknown>, success: string, close?: boolean) => Promise<void>;

/** One share in People with access: the person's photo, or a team's or organization's tile, then what they may do. */
function GrantRow({ grant, endpoint, busy, minutes, act }: { grant: Grant; endpoint: string; busy: boolean; minutes: string; act: Act }) {
  const type = grant.principal.type;
  const isPerson = type === "carbon" || type === "silicon";
  const person = usePerson({ uuid: grant.principal.id, id: grant.principal.public_id || grant.principal.id, kind: type === "silicon" ? "silicon" : "carbon" }, isPerson);
  const who = isPerson ? person.display_name?.trim() || person.id : grant.principal.name || grant.principal.public_id || grant.principal.id;
  const what = isPerson ? person.id : type === "team" ? "Team" : "Organization";
  return (
    <Row wide
      lead={isPerson ? <PersonAvatar person={person} /> : <Tile>{type === "team" ? <UsersRound size={16} strokeWidth={2} /> : <Building2 size={16} strokeWidth={2} />}</Tile>}
      title={who}
      detail={`${what} · ${accessLabel(grant.access)} · ${grant.expires_at ? `Until ${date(grant.expires_at)}` : "No expiry"}`}>
      {grant.expires_at ? <Button variant="secondary" size="sm" disabled={busy} onClick={() => void act(() => api.patch(`${endpoint}/invitations/${grant.id}`, minutes ? { expires_in_minutes: Number(minutes) } : { permanent: true }), minutes ? "Expiry changed" : "Share made permanent")}>{minutes ? "Set New Expiry" : "Keep Sharing"}</Button> : null}
      <Button variant="ghost" size="sm" disabled={busy} aria-label={`Remove access for ${who}`} onClick={() => void act(() => api.delete(`${endpoint}/invitations/${grant.id}`), "Access removed")}>Remove</Button>
    </Row>
  );
}

export function EntryDetail({ entry: initial, onClose, onChanged }: { entry: Entry; onClose: () => void; onChanged: () => void }) {
  const cache = useQueryClient();
  const [tab, setTab] = useState("preview");
  const [name, setName] = useState(initial.name);
  const [destination, setDestination] = useState<string | null>(null);
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
  const act: Act = async (work, success, close = false) => {
    setBusy(true); setFailure("");
    try { await work(); notify.success(success); await cache.invalidateQueries({ queryKey: ["entry", entry.id] }); await cache.invalidateQueries({ queryKey: ["grants", entry.id] }); await cache.invalidateQueries({ queryKey: ["link", entry.id] }); await cache.invalidateQueries({ queryKey: ["versions", entry.id] }); onChanged(); if (close) onClose(); }
    catch (error) { setFailure(message(error)); } finally { setBusy(false); }
  };
  const canUpdate = entry.effective_access.includes("update");
  const canDelete = entry.effective_access.includes("delete");
  async function move() {
    const parent = destination ?? (await api.get<Entry>(`/v1/paths/${encodeURIComponent(entry.org_id)}${selectedFolder ? `/${selectedFolder.split("/").map(encodeURIComponent).join("/")}` : ""}`)).id;
    await api.patch(endpoint, { parent_id: parent }, { idempotencyKey: true }); setMoving(false);
  }
  const grants = invitations.data?.pages.flatMap(page => page.items) ?? [];
  const versionItems = versions.data?.pages.flatMap(page => page.items) ?? [];
  const tabs = TABS.filter(value => value !== "versions" || entry.type === "file");
  const description = entry.type === "folder" ? "Folder details and sharing" : `${kindOf(entry).label} · ${size(entry.size)}${entry.content_type ? ` · ${entry.content_type}` : ""}`;

  return <Dialog open onOpenChange={open => { if (!open) onClose(); }}>
    <DialogContent className={entry.deleted_at ? undefined : styles.dialog} placement="top" title={entry.name} description={description}>
      {entry.deleted_at ? (
        <div className={styles.stack}>
          {failure ? <Alert tone="danger" title="That did not work" onDismiss={() => setFailure("")}>{failure}</Alert> : null}
          <p className={styles.text}>This entry was moved to the bin on {date(entry.deleted_at)}. Restore it before its 45-day retention ends.</p>
          <div className={styles.actions}>
            <Button variant="ghost" onClick={onClose}>Cancel</Button>
            <Button loading={busy} onClick={() => void act(() => api.post(`/v1/bin/${entry.id}/restore`, {}), "Entry restored", true)}><RotateCcw size={16} strokeWidth={2} aria-hidden="true" />Restore from Bin</Button>
          </div>
        </div>
      ) : (
        <Tabs value={tab} onValueChange={value => { setTab(value); setFailure(""); }} className={styles.tabs}>
          <TabsList aria-label="File details">
            {tabs.map(value => <TabsTrigger key={value} value={value}>{LABELS[value]}</TabsTrigger>)}
          </TabsList>
          {failure ? <Alert tone="danger" title="That did not work" onDismiss={() => setFailure("")}>{failure}</Alert> : null}

          <TabsContent value="preview" forceMount className={`${styles.panel} ${styles.previewPanel}`}>
            <Preview entry={entry} fill />
            <div className={styles.actions}>
              <ButtonLink href={new URL(entry.permanent_url).pathname} variant="ghost" size="sm"><ExternalLink size={16} strokeWidth={2} aria-hidden="true" />Open Permanent Address</ButtonLink>
              <ButtonLink href={contentUrl(entry.id, true)} variant="secondary" size="sm"><Download size={16} strokeWidth={2} aria-hidden="true" />Download {entry.type === "folder" ? "Folder" : "File"}</ButtonLink>
            </div>
          </TabsContent>

          <TabsContent value="sharing" forceMount className={styles.panel}>
            {link.error ? <ErrorAlert error={link.error} /> : null}
            {invitations.error ? <ErrorAlert error={invitations.error} /> : null}
            {link.isPending && tab === "sharing" ? <SectionSkeleton rows={2} title label="Loading sharing" /> : null}
            {link.data?.can_manage ? <ShareAccess endpoint={endpoint} folder={entry.type === "folder"} busy={busy} minutes={minutes} setMinutes={setMinutes} act={act} /> : null}
            {link.data?.can_manage && grants.length ? (
              <Section id="people-with-access" title="People with access" count={grants.length}>
                {grants.map(grant => <GrantRow key={grant.id} grant={grant} endpoint={endpoint} busy={busy} minutes={minutes} act={act} />)}
              </Section>
            ) : null}
            {invitations.hasNextPage ? <Button variant="secondary" size="sm" loading={invitations.isFetchingNextPage} onClick={() => void invitations.fetchNextPage()}>More Shares</Button> : null}
            {link.data ? (
              <Section id="link-access" title="Anyone with the link">
                <Row wide lead={<Tile><Globe size={16} strokeWidth={2} /></Tile>} title={link.data.effective ? "On" : "Off"} detail={link.data.effective ? "Anyone with the link can open it without signing in." : "Only accounts with access can open it."}>
                  {link.data.can_manage ? <Button variant={link.data.enabled ? "ghost" : "secondary"} size="sm" loading={busy} onClick={() => void act(() => api.put(`${endpoint}/link-access`, { enabled: !link.data?.enabled, ...(!link.data?.enabled && minutes ? { expires_in_minutes: Number(minutes) } : {}) }), link.data?.enabled ? "Link access turned off" : "Link access enabled")}>{link.data.enabled ? "Turn Off Link Access" : "Enable Link Access"}</Button> : null}
                </Row>
              </Section>
            ) : null}
            {link.data?.url ? (
              <div className={styles.address}>
                <Input label="Shareable address" readOnly value={link.data.url} onFocus={event => event.target.select()} />
                <CopyButton value={link.data.url} label="Copy Link" />
              </div>
            ) : null}
          </TabsContent>

          {entry.type === "file" ? (
            <TabsContent value="versions" forceMount className={styles.panel}>
              {versions.error ? <ErrorAlert error={versions.error} /> : null}
              {versionItems.length ? (
                <Section id="versions" label="Versions">
                  {versionItems.map((version, index) => (
                    <Row key={version.id} lead={<Tile><History size={16} strokeWidth={2} /></Tile>} title={date(version.created_at)} detail={`${size(version.size)}${version.created_by?.id ? ` · ${version.created_by.id}` : ""}`}>
                      {index === 0 ? <Badge size="sm" tone="neutral">Current</Badge>
                        : canUpdate ? <Button variant="secondary" size="sm" loading={busy} onClick={() => void act(() => api.post(`${endpoint}/versions/${version.id}/restore`, {}), "Version restored")}>Restore Version</Button> : null}
                    </Row>
                  ))}
                </Section>
              ) : versions.data ? <p className={styles.text}>No earlier versions are available.</p> : versions.error ? null : <SectionSkeleton rows={2} label="Loading versions" />}
              {versions.hasNextPage ? <Button variant="secondary" size="sm" loading={versions.isFetchingNextPage} onClick={() => void versions.fetchNextPage()}>More Versions</Button> : null}
            </TabsContent>
          ) : null}

          <TabsContent value="activity" forceMount className={styles.panel}>
            {activity.error ? <ErrorAlert error={activity.error} /> : null}
            {activity.data?.items.length ? (
              <Section id="activity" label="Activity">
                {activity.data.items.map((event, index) => {
                  const what = (event.action || event.event || event.kind || "File updated").replaceAll("_", " ");
                  return <Row key={event.id || index} lead={<Tile><History size={16} strokeWidth={2} /></Tile>} title={what[0].toUpperCase() + what.slice(1)} detail={`${event.actor?.id ? `${event.actor.id} · ` : ""}${date(event.occurred_at)}`} />;
                })}
              </Section>
            ) : activity.data ? <p className={styles.text}>No activity to show.</p> : activity.error ? null : <SectionSkeleton rows={3} label="Loading activity" />}
          </TabsContent>

          <TabsContent value="details" forceMount className={styles.panel}>
            <div data-sq="surface" className={styles.facts}>
              <DescriptionList>
                <DescriptionItem label="Location">{entry.path || "/"}</DescriptionItem>
                <DescriptionItem label="Owner">{entry.owner?.id || "—"}</DescriptionItem>
                <DescriptionItem label="Created">{date(entry.created_at)}</DescriptionItem>
                <DescriptionItem label="Updated">{date(entry.updated_at)}</DescriptionItem>
                <DescriptionItem label="Your access">{entry.effective_access.join(", ")}</DescriptionItem>
                {entry.origin_app_id ? <DescriptionItem label="Created by app">{entry.origin_app_id}</DescriptionItem> : null}
              </DescriptionList>
            </div>
            {canUpdate ? (
              <section className={styles.group} aria-labelledby="entry-name-title">
                <h2 id="entry-name-title" className={styles.groupTitle}>Name and place</h2>
                <form data-sq="surface" className={styles.box} onSubmit={event => { event.preventDefault(); void act(() => api.patch(endpoint, { name: name.trim() }, { idempotencyKey: true }), "Name updated"); }}>
                  <Input label="Name" value={name} onChange={event => setName(event.target.value)} required />
                  <div className={styles.actions}>
                    <Button type="button" size="sm" variant="ghost" aria-expanded={moving} onClick={() => setMoving(!moving)}><Folder size={16} strokeWidth={2} aria-hidden="true" />Move to Folder</Button>
                    <Button type="submit" size="sm" variant="secondary" loading={busy} disabled={!name.trim() || name.trim() === entry.name}>Rename</Button>
                  </div>
                </form>
              </section>
            ) : null}
            {moving ? (
              <Section id="move" title="Move to a folder" description={selectedFolder ? <>Into <strong>{selectedFolder}</strong></> : "Choose a folder in this drive, or its top level."}>
                <ActionItem onClick={() => { setSelectedFolder(""); setDestination(null); }} lead={<Tile><Folder size={16} strokeWidth={2} /></Tile>} title="Drive root" detail="The top level of this drive" />
                {folderQuery.data?.pages.flatMap(page => page.items).filter(item => item.type === "folder" && item.id !== entry.id).map(item => (
                  <ActionItem key={item.id} onClick={() => { setSelectedFolder(item.path); setDestination(item.id); }} lead={<Tile><Folder size={16} strokeWidth={2} /></Tile>} title={item.name} detail={item.path} />
                ))}
              </Section>
            ) : null}
            {moving && folderQuery.error ? <ErrorAlert error={folderQuery.error} /> : null}
            {moving ? (
              <div className={styles.actions}>
                {folderQuery.hasNextPage ? <Button variant="ghost" size="sm" loading={folderQuery.isFetchingNextPage} onClick={() => void folderQuery.fetchNextPage()}>More Folders</Button> : null}
                <Button variant="ghost" size="sm" onClick={() => setMoving(false)}>Cancel</Button>
                <Button size="sm" loading={busy} onClick={() => void act(move, "Entry moved")}>Move Here</Button>
              </div>
            ) : null}
            {entry.self_destruct_at ? (
              <Alert tone="warning" title={`Deletes ${date(entry.self_destruct_at)}`}>
                This file deletes permanently then. Removing it early also skips the bin.
                <span className={styles.alertAction}><Button variant="secondary" size="sm" loading={busy} onClick={() => void act(() => api.delete(`${endpoint}/self-destruct`), "File kept permanently")}>Keep This File</Button></span>
              </Alert>
            ) : null}
            {canDelete ? (
              <Section id="danger" title={entry.self_destruct_at ? "Delete" : "Move to bin"} tone="danger">
                <Row wide lead={<Tile><Trash2 size={16} strokeWidth={2} /></Tile>} title={entry.self_destruct_at ? "Delete permanently" : "Move to the bin"} detail={entry.self_destruct_at ? "It skips the bin and cannot be restored." : "It stays in the bin for 45 days."}>
                  <HoldToConfirm label={entry.self_destruct_at ? "Hold to delete permanently" : "Hold to move to bin"} tone="danger" disabled={busy} onConfirm={() => void act(() => api.delete(endpoint), entry.self_destruct_at ? "File deleted permanently" : "Moved to bin", true)} />
                </Row>
              </Section>
            ) : null}
          </TabsContent>
        </Tabs>
      )}
    </DialogContent>
  </Dialog>;
}
