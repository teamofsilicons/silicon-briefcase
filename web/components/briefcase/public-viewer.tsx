"use client";

/**
 * A shared link, opened by anyone: the workspace's frame without the section list (PublicFrame), the page bar with the
 * entry's name and Download, then its preview or, for a folder, its contents as one boxed list.
 */
import { useInfiniteQuery } from "@tanstack/react-query";
import { Download } from "lucide-react";
import { Alert } from "@teamofsilicons/silicon-ui/alert";
import { Button } from "@teamofsilicons/silicon-ui/button";
import { ButtonLink } from "@teamofsilicons/silicon-ui/button-link";
import { EmptyState } from "@teamofsilicons/silicon-ui/empty-state";
import { Page } from "@/components/foundation/layout/layout";
import { PageBar } from "@/components/foundation/page/page-bar";
import { LinkItem, Section } from "@/components/foundation/page/rows";
import { PublicFrame } from "@/components/foundation/shell/public-frame";
import { Preview } from "./preview";
import { FileTile, kindOf } from "./file-kind";
import { size, type Entry } from "@/lib/briefcase/types";
import styles from "./inbox.module.css";

export type PublicEntry = { id: string; name: string; path: string; entry_type: "file" | "folder"; size: number | null; content_type: string | null };

export function PublicViewer({ entry, owner }: { entry: PublicEntry; owner: string }) {
  const route = (path: string) => `/public-api/${encodeURIComponent(owner)}/${path.split("/").map(encodeURIComponent).join("/")}`;
  const contents = useInfiniteQuery({ queryKey: ["public-contents", owner, entry.path], initialPageParam: null as string | null, enabled: entry.entry_type === "folder", queryFn: async ({ pageParam }) => { const response = await fetch(`${route(entry.path)}?view=contents${pageParam ? `&cursor=${encodeURIComponent(pageParam)}` : ""}`); if (!response.ok) throw new Error("The shared folder could not be loaded."); return await response.json() as { items: PublicEntry[]; next_cursor: string | null }; }, getNextPageParam: last => last.next_cursor ?? undefined });
  const extension = entry.name.split(".").pop()?.toLowerCase() || "";
  const render = entry.content_type?.startsWith("image/") ? "image" : entry.content_type?.startsWith("video/") ? "video" : entry.content_type?.startsWith("audio/") ? "audio" : ["json", "js", "ts", "py", "rs", "html", "css"].includes(extension) ? "code" : "document";
  const preview: Entry = { ...entry, type: entry.entry_type, render, updated_at: null, org_id: owner, parent_id: null, visibility: "full", created_at: null, deleted_at: null, self_destruct_at: null, owner: null, origin_app_id: null, effective_access: ["read"], permanent_url: `/${owner}/${entry.path}` };
  const items = contents.data?.pages.flatMap(page => page.items) ?? [];
  const asEntry = (item: PublicEntry) => ({ type: item.entry_type, name: item.name, content_type: item.content_type });
  return (
    <PublicFrame end={<ButtonLink href={`/auth/sign-in?return_to=${encodeURIComponent(`/${owner}/${entry.path}`)}`} variant="secondary" size="sm">Sign In</ButtonLink>}>
      <PageBar title={entry.name} actions={<ButtonLink href={`${route(entry.path)}?view=attachment`} size="sm" variant={entry.entry_type === "folder" ? "secondary" : "primary"}><Download size={16} strokeWidth={2} aria-hidden="true" />Download {entry.entry_type === "folder" ? "Folder" : "File"}</ButtonLink>} />
      <Page>
        <p className={styles.lead}>Shared from <strong>{owner}</strong>{entry.entry_type === "file" ? ` · ${kindOf(asEntry(entry)).label} · ${size(entry.size)}` : ""}. Anyone with this link can view and download it.</p>
        {entry.entry_type === "folder" ? <>
          {contents.error ? <Alert tone="danger" title="The shared folder could not be loaded">{contents.error.message}</Alert> : null}
          {items.length ? (
            <Section id="shared-contents" title="Inside" count={items.length}>
              {items.map(item => <LinkItem key={item.id} href={`/${encodeURIComponent(owner)}/${item.path.split("/").map(encodeURIComponent).join("/")}`} lead={<FileTile entry={asEntry(item)} />} title={item.name} detail={item.entry_type === "folder" ? "Folder" : `${kindOf(asEntry(item)).label} · ${size(item.size)}`} />)}
            </Section>
          ) : contents.data ? <div className={styles.empty}><EmptyState title="This folder is empty" description="Nothing has been added to it yet." /></div> : null}
          {contents.hasNextPage ? <Button variant="secondary" size="sm" loading={contents.isFetchingNextPage} onClick={() => void contents.fetchNextPage()}>Load More</Button> : null}
        </> : <Preview entry={preview} publicContentUrl={`${route(entry.path)}?view=inline`} />}
      </Page>
    </PublicFrame>
  );
}
