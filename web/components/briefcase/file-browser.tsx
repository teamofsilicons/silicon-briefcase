"use client";
import { useRef, useState } from "react";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { useRouter } from "next/navigation";
import { ChevronRight, File, Folder, FolderPlus, Grid2X2, List, Upload, X } from "lucide-react";
import { Button } from "@/components/arc/button/button";
import { Input } from "@/components/arc/input/input";
import { Select } from "@/components/arc/select/select";
import { Dialog, DialogContent } from "@/components/arc/dialog/dialog";
import { EmptyState } from "@/components/arc/empty-state/empty-state";
import { Page, PageHeader } from "@/components/foundation/layout/layout";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { api } from "@/lib/client/api";
import { notify } from "@/lib/notify";
import { date, message, size, type Drive, type Entry, type EntryPage, type SharedPage } from "@/lib/briefcase/types";
import { EntryDetail } from "./entry-detail";
import styles from "./briefcase.module.css";

type Mode = "files" | "shared" | "recent" | "bin";
type Transfer = { id: string; file: File; status: "uploading" | "done" | "failed"; error?: string; abort: AbortController; parent: string; drive: string; minutes: string };
export function FileBrowser({ mode = "files", accountUuid, drive = "", folder = "", initialDrives, initialEntry }: { mode?: Mode; accountUuid: string; drive?: string; folder?: string; initialDrives?: { items: Drive[] }; initialEntry?: Entry }) {
  const router = useRouter();
  const cache = useQueryClient();
  const [layout, setLayout] = useState<"list" | "grid">("list");
  const [search, setSearch] = useState("");
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("");
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
    queryKey: ["files", mode, drive, folder, query, filter], initialPageParam: null as string | null,
    queryFn: async ({ pageParam, signal }): Promise<EntryPage> => {
      if (query) {
        const result = await api.get<{ items: { entry: Entry }[] }>("/v1/search", { query: { q: query, limit: 20 }, signal });
        return { items: result.items.map(item => item.entry), next_cursor: null };
      }
      if (mode === "shared") {
        const result = await api.get<SharedPage>("/v1/shared", { query: { cursor: pageParam }, signal });
        return { items: result.items.map(item => item.entry), next_cursor: result.next_cursor };
      }
      return api.get<EntryPage>(mode === "bin" ? "/v1/bin" : "/v1/entries", {
        query: { drive, cursor: pageParam, ...(mode !== "bin" ? { path: mode === "files" && !filter ? folder : undefined, filter: mode === "recent" ? "last:100" : filter } : {}) }, signal,
      });
    }, getNextPageParam: last => last.next_cursor ?? undefined,
  });
  const entries = pages.data?.pages.flatMap(page => page.items) ?? [];
  const canCreate = mode === "files" && (!folder ? !!activeDrive : currentFolder.data?.effective_access.includes("write"));
  const title = query ? "Search results" : mode === "shared" ? "Shared with me" : mode === "recent" ? "Recent" : mode === "bin" ? "Bin" : folder.split("/").filter(Boolean).pop() || (activeDrive?.relation === "custodial" ? `${activeDrive.owner.display_name || activeDrive.owner.id}’s files` : "My files");
  function navigate(path: string, selectedDrive = drive) {
    const params = new URLSearchParams(); if (path) params.set("path", path); if (selectedDrive) params.set("drive", selectedDrive);
    router.push(`/files${params.size ? `?${params}` : ""}`);
  }
  function open(entry: Entry) { if (entry.type === "folder" && mode !== "bin") navigate(entry.path, entry.org_id); else setSelected(entry); }
  async function createFolder(event: React.FormEvent) {
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
  return <Page className={styles.filesPage}>
    <PageHeader title={title} description={mode === "bin" ? "Deleted files stay here for 45 days. Self-destructing files are removed permanently." : mode === "shared" ? "Files and folders others chose to share with you." : mode === "recent" ? "The latest changes across your drive." : "A little more room for everything you’re working on."} actions={canCreate ? <div className={styles.actions}><Button variant="secondary" onClick={() => setNewFolder(true)}><FolderPlus size={17} />New folder</Button><Button onClick={() => fileInput.current?.click()}><Upload size={17} />Upload files</Button></div> : undefined} />
    <input ref={fileInput} type="file" multiple hidden onChange={event => { if (event.target.files) upload(event.target.files); event.target.value = ""; }} />
    {mode === "files" && <nav className={styles.crumbs} aria-label="Folder path"><button onClick={() => navigate("")}>{activeDrive?.owner.id || "My drive"}</button>{folder.split("/").filter(Boolean).map((part, index, parts) => <span className={styles.controls} key={index}><ChevronRight size={13} /><button onClick={() => navigate(parts.slice(0,index + 1).join("/"))}>{part}</button></span>)}</nav>}
    {folder && currentFolder.data && <Button variant="ghost" size="sm" onClick={() => setSelected(currentFolder.data)}>Folder details and sharing</Button>}
    {drive && drive !== accountUuid && <p data-sq="surface" className={styles.note}>You might not be seeing all the contents of this folder.</p>}
    <div className={styles.toolbar}>
      <form className={styles.search} onSubmit={event => { event.preventDefault(); setQuery(search.trim()); }}><Input label="Search files" placeholder="Find a file, or something inside it…" value={search} onChange={event => setSearch(event.target.value)} /><button type="submit" className="sr-only">Search</button></form>
      {mode === "files" && <Select label="Show" value={filter || "all"} onValueChange={value => setFilter(value === "all" ? "" : value)} options={[{value:"all",label:"This folder"},{value:"is:folder",label:"All folders"},{value:"is:image",label:"Images"},{value:"is:document",label:"Documents"},{value:"is:self-destruct",label:"Self-destructing"},{value:"is:expiring",label:"Expiring shares"}]} />}
      <div className={styles.controls}><Button variant="ghost" aria-label="List view" aria-pressed={layout === "list"} onClick={() => setLayout("list")}><List size={18} /></Button><Button variant="ghost" aria-label="Grid view" aria-pressed={layout === "grid"} onClick={() => setLayout("grid")}><Grid2X2 size={18} /></Button></div>
    </div>
    {canCreate && <details className={styles.muted}><summary>Upload options</summary><div className={styles.toolbar}><Input label="Delete uploads permanently after (minutes)" type="number" min={1} max={43200} placeholder="Keep permanently" value={uploadMinutes} onChange={event => setUploadMinutes(event.target.value)} /></div></details>}
    {query && <p data-sq="surface" className={styles.note}>Results for “{query}” across files you can access. <Button size="sm" variant="ghost" onClick={() => { setQuery(""); setSearch(""); }}>Clear search</Button></p>}
    {transfers.length > 0 && <div data-sq="surface" className={styles.uploads} aria-live="polite">{transfers.map(item => <div key={item.id} className={styles.uploadRow}><span>{item.file.name}<small className={styles.muted}> · {size(item.file.size)} {item.error}</small></span><span>{item.status === "done" ? "Uploaded" : item.status === "failed" ? "Failed" : "Uploading…"}</span>{item.status === "failed" && <Button size="sm" variant="secondary" onClick={() => void transfer({ ...item, abort: new AbortController() })}>Retry</Button>}{item.status === "uploading" ? <Button size="sm" variant="ghost" aria-label={`Cancel ${item.file.name}`} onClick={() => item.abort.abort()}><X size={14} /></Button> : <Button size="sm" variant="ghost" aria-label={`Dismiss ${item.file.name}`} onClick={() => setTransfers(previous => previous.filter(t => t.id !== item.id))}><X size={14} /></Button>}</div>)}</div>}
    {pages.error && <ErrorAlert error={pages.error} action={<Button variant="secondary" onClick={() => void pages.refetch()}>Try again</Button>} />}
    {currentFolder.error && <ErrorAlert error={currentFolder.error} />}
    <div className={dragging ? styles.drop : ""} onDragOver={event => { event.preventDefault(); if (canCreate) setDragging(true); }} onDragLeave={() => setDragging(false)} onDrop={event => { event.preventDefault(); setDragging(false); upload(event.dataTransfer.files); }}>
      {pages.isPending ? <div className={styles.empty} role="status">Loading your files…</div> : entries.length === 0 ? <EmptyState title={query || filter ? "No matching files" : mode === "bin" ? "Nothing in the bin" : mode === "shared" ? "A place for shared work" : "Make room for something good"} description={query || filter ? "Try another search or clear the filters." : mode === "files" ? "Drop files here, upload a document, or create your first folder." : "Files will appear here when there’s something to show."} action={canCreate ? <Button onClick={() => fileInput.current?.click()}>Upload files</Button> : undefined} /> : layout === "grid" ? <div className={styles.grid}>{entries.map(entry => <button className={styles.tile} data-sq="surface" key={entry.id} onClick={() => open(entry)}><span data-sq="surface" className={styles.fileIcon}>{entry.type === "folder" ? <Folder size={27} /> : <File size={25} />}</span><strong>{entry.name}</strong><small>{entry.type === "folder" ? "Folder" : size(entry.size)}</small></button>)}</div> : <div className={styles.surface} data-sq="surface"><table className={styles.table}><thead><tr><th>Name</th><th className={styles.hideMobile}>Updated</th><th>Size</th><th><span className="sr-only">Actions</span></th></tr></thead><tbody>{entries.map(entry => <tr key={entry.id}><td><button className={styles.rowButton} onClick={() => open(entry)}><span data-sq="surface" className={styles.fileIcon}>{entry.type === "folder" ? <Folder size={21} /> : <File size={20} />}</span><span className={styles.fileName}><strong>{entry.name}</strong><small>{entry.self_destruct_at ? `Deletes ${date(entry.self_destruct_at)}` : mode !== "files" ? entry.path : entry.origin_app_id ? `Created by ${entry.origin_app_id}` : entry.type === "folder" ? "Folder" : entry.content_type}</small></span></button></td><td className={styles.hideMobile}>{date(entry.updated_at)}</td><td>{entry.type === "folder" ? "—" : size(entry.size)}</td><td><Button variant="ghost" size="sm" aria-label={`Details for ${entry.name}`} onClick={() => setSelected(entry)}>Details</Button></td></tr>)}</tbody></table></div>}
    </div>
    <div className={styles.footer}><span className={styles.muted}>{entries.length} {entries.length === 1 ? "item" : "items"}</span>{pages.hasNextPage && <Button variant="secondary" loading={pages.isFetchingNextPage} onClick={() => void pages.fetchNextPage()}>Load more</Button>}</div>
    <Dialog open={newFolder} onOpenChange={setNewFolder}><DialogContent title="New folder" description="Give your work a place to grow."><form onSubmit={createFolder} className={styles.form}><Input label="Folder name" value={folderName} onChange={event => setFolderName(event.target.value)} required autoFocus />{failure && <p role="alert" data-sq="surface" className={styles.error}>{failure}</p>}<Button type="submit" loading={busy}>Create folder</Button></form></DialogContent></Dialog>
    {selected && <EntryDetail key={selected.id} entry={selected} onClose={() => setSelected(null)} onChanged={() => { void cache.invalidateQueries({ queryKey: ["files"] }); }} />}
  </Page>;
}
