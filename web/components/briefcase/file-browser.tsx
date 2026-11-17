"use client";

/**
 * My files, Shared with me, Recent and the Bin, drawn as Silicon Accounts draws its pages: the page bar (the folder's
 * path above its name, the page's buttons at its right), then the page column with a line of context, search and
 * filters, and the entries as one boxed list of rows (a 28px tile for the kind, the name over one detail line, Details at
 * the end), or as cards in the grid view. A file's details open in EntryDetail.
 */
import Link from "next/link";
import { useRef, useState, type FormEvent } from "react";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { useRouter } from "next/navigation";
import { Check, FolderPlus, Info, LoaderCircle, Upload, X } from "lucide-react";
import { Alert } from "@teamofsilicons/silicon-ui/alert";
import { Button } from "@teamofsilicons/silicon-ui/button";
import { Input } from "@teamofsilicons/silicon-ui/input";
import { Select } from "@teamofsilicons/silicon-ui/select";
import SegmentedControl from "@teamofsilicons/silicon-ui/segmented-control";
import { Dialog, DialogContent } from "@teamofsilicons/silicon-ui/dialog";
import { EmptyState } from "@teamofsilicons/silicon-ui/empty-state";
import { SkeletonBlock } from "@/components/foundation/feedback/skeleton-block";
import { Page } from "@/components/foundation/layout/layout";
import { PageBar, type Crumb } from "@/components/foundation/page/page-bar";
import { ActionItem, Row, Section, Tile } from "@/components/foundation/page/rows";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { api } from "@/lib/client/api";
import { notify } from "@/lib/notify";
import { date, message, size, type Drive, type Entry, type EntryPage, type SharedPage } from "@/lib/briefcase/types";
import { usePeople } from "./people-query";
import { PersonLabel } from "./person-label";
import { useOrganizations } from "./organizations-query";
import { EntryDetail } from "./entry-detail";
import { FileTile, kindOf } from "./file-kind";
import styles from "./files.module.css";

type Mode = "files" | "shared" | "recent" | "bin";
type Transfer = { id: string; file: File; status: "uploading" | "done" | "failed"; error?: string; abort: AbortController; parent: string; drive: string; minutes: string };

/** One line under an entry's name: what it is, and what matters about it here. */
function detailFor(entry: Entry, mode: Mode) {
  if (entry.self_destruct_at) return `Deletes ${date(entry.self_destruct_at)}`;
  if (mode === "bin") return `Deleted ${date(entry.deleted_at)} · ${entry.path}`;
  if (mode !== "files") return entry.path;
  const what = entry.type === "folder" ? "Folder" : `${kindOf(entry).label} · ${size(entry.size)}`;
  return entry.origin_app_id ? `${what} · Created by ${entry.origin_app_id}` : what;
}

export function FileBrowser({ mode = "files", accountUuid, drive = "", folder = "", initialDrives, initialEntry, sharedFrom = "", sharedContacts = "", sharedTrust = "" }: { mode?: Mode; accountUuid: string; drive?: string; folder?: string; initialDrives?: { items: Drive[] }; initialEntry?: Entry; sharedFrom?: string; sharedContacts?: string; sharedTrust?: string }) {
  const router = useRouter();
  const organizations = useOrganizations();
  const work = organizations.data?.organizations[0];
  const cache = useQueryClient();
  const [layout, setLayout] = useState<"list" | "grid">("list");
  const [search, setSearch] = useState("");
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("");
  const [from, setFrom] = useState(sharedFrom);
  const [contactFilter, setContactFilter] = useState(sharedContacts);
  const [minTrust, setMinTrust] = useState(sharedTrust);
  const people = usePeople(mode === "shared");
  const [selected, setSelected] = useState<Entry | null>(initialEntry ?? null);
  const [newFolder, setNewFolder] = useState(false);
  const [folderName, setFolderName] = useState("");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const [dragging, setDragging] = useState(false);
  const [transfers, setTransfers] = useState<Transfer[]>([]);
  const [uploadMinutes, setUploadMinutes] = useState("");
  const fileInput = useRef<HTMLInputElement>(null);
  const drives = useQuery({ queryKey: ["drives"], queryFn: () => api.get<{ items: Drive[] }>("/v1/drives"), initialData: initialDrives });
  const activeDrive = drives.data?.items.find(item => item.owner.uuid === drive || item.owner.id === drive || (!drive && item.relation === "own"));
  const currentFolder = useQuery({ queryKey: ["folder", drive, folder], queryFn: () => api.get<Entry>(`/v1/paths/${encodeURIComponent(drive || accountUuid)}/${folder.split("/").map(encodeURIComponent).join("/")}`), enabled: !!folder });
  const pages = useInfiniteQuery({
    queryKey: ["files", mode, accountUuid, drive, folder, query, filter, from, contactFilter, minTrust], initialPageParam: null as string | null,
    queryFn: async ({ pageParam, signal }): Promise<EntryPage> => {
      if (query && mode !== "shared") {
        const result = await api.get<{ items: { entry: Entry }[] }>("/v1/search", { query: { q: query, limit: 20 }, signal });
        return { items: result.items.map(item => item.entry), next_cursor: null };
      }
      if (mode === "shared") {
        if (minTrust && (!Number.isInteger(Number(minTrust)) || Number(minTrust) < 1 || Number(minTrust) > 255)) throw new Error("Minimum trust must be a whole number from 1 to 255.");
        const result = await api.get<SharedPage>("/v1/shared", { query: { cursor: pageParam, from: from || undefined, contacts: contactFilter || undefined, min_trust: minTrust || undefined, q: query || undefined }, signal });
        return { items: result.items.map(item => ({ ...item.entry, shared_by: item.shared_by ?? [item.owner] })), next_cursor: result.next_cursor };
      }
      return api.get<EntryPage>(mode === "bin" ? "/v1/bin" : "/v1/entries", {
        query: { drive, cursor: pageParam, ...(mode !== "bin" ? { path: mode === "files" && !filter ? folder : undefined, filter: mode === "recent" ? "last:100" : filter } : {}) }, signal,
      });
    }, getNextPageParam: last => last.next_cursor ?? undefined,
  });
  const entries = pages.data?.pages.flatMap(page => page.items) ?? [];
  const canCreate = mode === "files" && (!folder ? !!activeDrive : currentFolder.data?.effective_access.includes("write"));
  const driveName = activeDrive?.relation === "custodial" ? `${activeDrive.owner.display_name || activeDrive.owner.id}’s files` : work ? "My work files" : "My personal files";
  const parts = folder.split("/").filter(Boolean);
  const title = mode === "shared" ? "Shared with me" : query ? "Search results" : mode === "recent" ? "Recent" : mode === "bin" ? "Bin" : parts.at(-1) || driveName;
  const description = mode === "bin" ? "Deleted files stay here for 45 days. Self-destructing files are removed permanently." : mode === "shared" ? "Files shared directly with you, your teams, and your organization." : mode === "recent" ? "The latest changes across your drive." : work ? `Your files in ${work.name}. Share with people or teams when you’re ready.` : "Your personal space. Files stay private until you choose to share them.";
  const filtered = !!(query || filter || from || contactFilter || minTrust);

  function navigate(path: string, selectedDrive = drive) {
    const params = new URLSearchParams(); if (path) params.set("path", path); if (selectedDrive) params.set("drive", selectedDrive);
    router.push(`/files${params.size ? `?${params}` : ""}`);
  }
  // The folders above this one, then the drive: the page bar's way back (the folder itself is the title).
  const trail: Crumb[] = mode === "files" && !query && parts.length ? [{ label: driveName, onClick: () => navigate("") }, ...parts.slice(0, -1).map((part, index) => ({ label: part, onClick: () => navigate(parts.slice(0, index + 1).join("/")) }))] : [];
  function open(entry: Entry) { if (entry.type === "folder" && mode !== "bin") navigate(entry.path, entry.org_id); else setSelected(entry); }
  async function createFolder(event: FormEvent) {
    event.preventDefault(); setBusy(true); setFailure("");
    try {
      await api.post("/v1/entries", { name: folderName.trim(), parent_id: currentFolder.data?.id ?? activeDrive?.root_id, invitees: [] }, { idempotencyKey: true });
      setNewFolder(false); setFolderName(""); await cache.invalidateQueries({ queryKey: ["files"] }); notify.success("Folder created");
    } catch (error) { setFailure(message(error)); } finally { setBusy(false); }
  }
  async function transfer(item: Transfer) {
    const body = new FormData(); body.set("file", item.file, item.file.name);
    if (item.parent) body.set("parent_id", item.parent); else if (item.drive) body.set("drive", item.drive);
    if (item.minutes) body.set("self_destruct_minutes", item.minutes);
    setTransfers(previous => previous.map(t => t.id === item.id ? { ...t, status: "uploading", error: undefined, abort: item.abort } : t));
    try {
      await api.post("/v1/uploads", undefined, { raw: body, signal: item.abort.signal, idempotencyKey: item.id });
      setTransfers(previous => previous.map(t => t.id === item.id ? { ...t, status: "done" } : t));
      await cache.invalidateQueries({ queryKey: ["files"] });
    } catch (error) { setTransfers(previous => previous.map(t => t.id === item.id ? { ...t, status: "failed", error: message(error) } : t)); }
  }
  function upload(files: FileList | File[]) {
    if (!canCreate) return;
    const added = Array.from(files).map(file => ({ id: crypto.randomUUID(), file, status: "uploading" as const, abort: new AbortController(), parent: currentFolder.data?.id ?? activeDrive?.root_id ?? "", drive, minutes: uploadMinutes }));
    setTransfers(previous => [...previous, ...added]);
    // Bound concurrent uploads while retaining a stable idempotency key for each retry.
    void (async () => { for (const item of added) await transfer(item); })();
  }
  const sharers = (entry: Entry) => mode === "shared" && entry.shared_by?.length ? (
    <span className={styles.sharers}>{entry.shared_by.map(person => <PersonLabel key={person.uuid} person={{ ...person, id: person.id ?? person.uuid }} />)}</span>
  ) : null;

  return <>
    <PageBar trail={trail} title={title} actions={
      <div className={styles.barActions}>
        {folder && currentFolder.data && mode === "files" ? <Button variant="ghost" size="sm" onClick={() => setSelected(currentFolder.data)} aria-label="Folder details and sharing"><Info size={16} strokeWidth={2} aria-hidden="true" />Details</Button> : null}
        <span className={styles.viewSwitch}><SegmentedControl size="xs" label="View" value={layout} onValueChange={value => setLayout(value === "grid" ? "grid" : "list")} options={[{ value: "list", label: "List" }, { value: "grid", label: "Grid" }]} /></span>
        {canCreate ? <>
          <Button variant="secondary" size="sm" onClick={() => setNewFolder(true)}><FolderPlus size={16} strokeWidth={2} aria-hidden="true" />New Folder</Button>
          <Button size="sm" onClick={() => fileInput.current?.click()}><Upload size={16} strokeWidth={2} aria-hidden="true" />Upload Files</Button>
        </> : null}
      </div>
    } />
    <Page width={layout === "grid" ? "default" : "narrow"} className={styles.page}>
      <input ref={fileInput} type="file" multiple hidden onChange={event => { if (event.target.files) upload(event.target.files); event.target.value = ""; }} />
      <p className={styles.lead}>{description}</p>
      <div className={styles.toolbar}>
        <form className={styles.search} onSubmit={event => { event.preventDefault(); setQuery(search.trim()); }}>
          <Input label="Search files" placeholder="Find a file, or something inside it…" value={search} onChange={event => setSearch(event.target.value)} />
          <button type="submit" className="sr-only">Search</button>
        </form>
        {mode === "files" ? <Select label="Show" value={filter || "all"} onValueChange={value => setFilter(value === "all" ? "" : value)} options={[{ value: "all", label: "This folder" }, { value: "is:folder", label: "All folders" }, { value: "is:image", label: "Images" }, { value: "is:document", label: "Documents" }, { value: "is:self-destruct", label: "Self-destructing" }, { value: "is:expiring", label: "Expiring shares" }]} /> : null}
      </div>
      {mode === "shared" ? <div className={styles.filters}>
        <Select label="Shared by" value={from || "all"} onValueChange={value => setFrom(value === "all" ? "" : value)} options={[{ value: "all", label: "Anyone" }, ...(from && !people.data?.items.some(p => p.uuid === from) ? [{ value: from, label: "Selected account" }] : []), ...(people.data?.items ?? []).map(person => ({ value: person.uuid, label: `${person.display_name || person.id} · ${person.id}` }))]} />
        <Select label="Contacts" value={contactFilter || "all"} onValueChange={value => { setContactFilter(value === "all" ? "" : value); if (value === "unsaved") setMinTrust(""); }} options={[{ value: "all", label: "Everyone" }, { value: "saved", label: "Saved contacts" }, { value: "unsaved", label: "Not in contacts" }]} disabled={people.data?.contacts_status !== "connected"} />
        <Input label="Minimum trust" type="number" min={1} max={255} step={1} placeholder="Any trust" value={minTrust} onChange={event => { setMinTrust(event.target.value); if (event.target.value) setContactFilter("saved"); }} disabled={people.data?.contacts_status !== "connected"} />
        {from || contactFilter || minTrust ? <Button variant="ghost" size="sm" onClick={() => { setFrom(""); setContactFilter(""); setMinTrust(""); }}>Clear People Filters</Button> : null}
      </div> : null}
      {mode === "shared" && people.data?.contacts_status === "disconnected" ? <p className={styles.hint}><Link href="/contacts">Connect Silicon Contacts</Link> to filter shared files by saved contacts and trust.</p> : null}
      {mode === "shared" && (people.error || people.data?.contacts_status === "unavailable") ? <p role="status" className={styles.hint}>Contact filters are unavailable. <Button variant="ghost" size="xs" onClick={() => void people.refetch()}>Retry Contacts</Button></p> : null}
      {drive && drive !== accountUuid ? <Alert tone="info" title="Some of this folder may be hidden">You might not be seeing all the contents of this folder.</Alert> : null}
      {canCreate ? <details className={styles.options}>
        <summary>Upload options</summary>
        <Input label="Delete uploads permanently after (minutes)" type="number" min={1} max={43200} placeholder="Keep permanently" value={uploadMinutes} onChange={event => setUploadMinutes(event.target.value)} />
      </details> : null}
      {query ? <div className={styles.results} role="status">
        <span>Results for “{query}” {mode === "shared" ? "within files shared with you" : "across files you can access"}.</span>
        <Button size="xs" variant="ghost" onClick={() => { setQuery(""); setSearch(""); }}>Clear Search</Button>
      </div> : null}
      {transfers.length > 0 ? <Section id="uploads" title="Uploads" count={transfers.length}>
        <span className="sr-only" aria-live="polite">{transfers.filter(item => item.status === "done").length} of {transfers.length} uploaded</span>
        {transfers.map(item => (
          <Row key={item.id} lead={<Tile>{item.status === "done" ? <Check size={16} strokeWidth={2} /> : item.status === "failed" ? <X size={16} strokeWidth={2} /> : <LoaderCircle className="spin" size={16} strokeWidth={2} />}</Tile>} title={item.file.name} detail={<>{size(item.file.size)} · <span>{item.status === "done" ? "Uploaded" : item.status === "failed" ? "Failed" : "Uploading…"}</span></>} error={item.error}>
            {item.status === "failed" ? <Button size="sm" variant="secondary" onClick={() => void transfer({ ...item, abort: new AbortController() })}>Retry</Button> : null}
            {item.status === "uploading"
              ? <Button size="sm" variant="ghost" aria-label={`Cancel ${item.file.name}`} onClick={() => item.abort.abort()}><X size={16} strokeWidth={2} aria-hidden="true" /></Button>
              : <Button size="sm" variant="ghost" aria-label={`Dismiss ${item.file.name}`} onClick={() => setTransfers(previous => previous.filter(t => t.id !== item.id))}><X size={16} strokeWidth={2} aria-hidden="true" /></Button>}
          </Row>
        ))}
      </Section> : null}
      {pages.error ? <ErrorAlert error={pages.error} action={<Button variant="secondary" size="sm" onClick={() => void pages.refetch()}>Try Again</Button>} /> : null}
      {currentFolder.error ? <ErrorAlert error={currentFolder.error} /> : null}
      <div className={styles.dropArea} data-dragging={dragging ? "" : undefined} onDragOver={event => { event.preventDefault(); if (canCreate) setDragging(true); }} onDragLeave={() => setDragging(false)} onDrop={event => { event.preventDefault(); setDragging(false); upload(event.dataTransfer.files); }}>
        {pages.isPending ? (
          <div className={styles.loading} role="status" aria-label="Loading your files">
            <SkeletonBlock width="100%" height="calc(4 * 3.25rem)" radius="var(--radius-container)" />
          </div>
        ) : entries.length === 0 ? (
          <div className={styles.empty}>
            <EmptyState title={filtered ? "No matching files" : mode === "bin" ? "Nothing in the bin" : mode === "shared" ? "A place for shared work" : "Make room for something good"} description={filtered ? "Try another search or clear the filters." : mode === "files" ? "Drop files here, upload a document, or create your first folder." : "Files will appear here when there’s something to show."} action={canCreate ? <Button size="sm" onClick={() => fileInput.current?.click()}>Upload Files</Button> : undefined} />
          </div>
        ) : layout === "grid" ? (
          <ul className={styles.grid} aria-label={title}>
            {entries.map(entry => (
              <li key={entry.id}>
                <button type="button" data-sq="surface" className={styles.card} onClick={() => open(entry)}>
                  <FileTile entry={entry} large />
                  <span className={styles.cardName}>{entry.name}</span>
                  <span className={styles.cardDetail}>{entry.type === "folder" ? "Folder" : size(entry.size)}</span>
                  {sharers(entry)}
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <Section id="entries" label={title}>
            {entries.map(entry => (
              <ActionItem key={entry.id} onClick={() => open(entry)} lead={<FileTile entry={entry} />} title={entry.name} detail={<>{detailFor(entry, mode)}{sharers(entry)}</>} end={<span className={styles.updated}>{date(entry.updated_at)}</span>}>
                <Button variant="ghost" size="sm" aria-label={`Details for ${entry.name}`} onClick={() => setSelected(entry)}>Details</Button>
              </ActionItem>
            ))}
          </Section>
        )}
      </div>
      {entries.length || pages.hasNextPage ? <div className={styles.listFoot}>
        <span>{entries.length} {entries.length === 1 ? "item" : "items"}</span>
        {pages.hasNextPage ? <Button variant="secondary" size="sm" loading={pages.isFetchingNextPage} onClick={() => void pages.fetchNextPage()}>Load More</Button> : null}
      </div> : null}
    </Page>
    <Dialog open={newFolder} onOpenChange={setNewFolder}>
      <DialogContent title="New Folder" description="Give your work a place to grow.">
        <form onSubmit={createFolder} className={styles.form}>
          <Input label="Folder name" value={folderName} onChange={event => setFolderName(event.target.value)} required autoFocus />
          {failure ? <Alert tone="danger" title="The folder was not created">{failure}</Alert> : null}
          <div className={styles.dialogActions}>
            <Button type="button" variant="ghost" onClick={() => setNewFolder(false)}>Cancel</Button>
            <Button type="submit" loading={busy}>Create Folder</Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
    {selected ? <EntryDetail key={selected.id} entry={selected} onClose={() => setSelected(null)} onChanged={() => { void cache.invalidateQueries({ queryKey: ["files"] }); }} /> : null}
  </>;
}
