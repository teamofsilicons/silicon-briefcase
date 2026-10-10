"use client";
import Link from "next/link";
import { useInfiniteQuery } from "@tanstack/react-query";
import { File, Folder } from "lucide-react";
import { Button } from "@/components/arc/button/button";
import { Page, PageHeader } from "@/components/foundation/layout/layout";
import { Preview } from "./preview";
import { size, type Entry } from "@/lib/briefcase/types";
import styles from "./briefcase.module.css";
export type PublicEntry = { id:string; name:string; path:string; entry_type:"file"|"folder"; size:number|null;content_type:string|null };
export function PublicViewer({ entry, owner }: {entry:PublicEntry;owner:string}) {
 const route = (path:string) => `/public-api/${encodeURIComponent(owner)}/${path.split("/").map(encodeURIComponent).join("/")}`;
 const contents=useInfiniteQuery({queryKey:["public-contents",owner,entry.path],initialPageParam:null as string|null,enabled:entry.entry_type==="folder",queryFn:async({pageParam}) => { const response=await fetch(`${route(entry.path)}?view=contents${pageParam ? `&cursor=${encodeURIComponent(pageParam)}`:""}`);if(!response.ok)throw new Error("The shared folder could not be loaded.");return await response.json() as {items:PublicEntry[];next_cursor:string|null}; },getNextPageParam:last=>last.next_cursor??undefined});
 const extension=entry.name.split(".").pop()?.toLowerCase()||"";
 const render=entry.content_type?.startsWith("image/")?"image":entry.content_type?.startsWith("video/")?"video":entry.content_type?.startsWith("audio/")?"audio":["json","js","ts","py","rs","html","css"].includes(extension)?"code":"document";
 const preview: Entry={...entry,type:entry.entry_type,render,updated_at:null,org_id:owner,parent_id:null,visibility:"full",created_at:null,deleted_at:null,self_destruct_at:null,owner:null,origin_app_id:null,effective_access:["read"],permanent_url:`/${owner}/${entry.path}`};
 return <main><Page width="reading"><div className={styles.toolbar}><Link href="/">Silicon Briefcase</Link><Link href={`/auth/sign-in?return_to=${encodeURIComponent(`/${owner}/${entry.path}`)}`} style={{marginLeft:"auto"}}>Sign in to your files</Link></div><PageHeader title={entry.name} description="Shared with anyone who has this link." actions={<a href={`${route(entry.path)}?view=attachment`}>Download {entry.entry_type === "folder" ? "folder":"file"}</a>} />{entry.entry_type === "folder" ? <>{contents.error&&<p role="alert">{contents.error.message}</p>}<ul className={styles.list}>{contents.data?.pages.flatMap(page=>page.items).map(item=><li key={item.id}><Link className={styles.rowButton} href={`/${encodeURIComponent(owner)}/${item.path.split("/").map(encodeURIComponent).join("/")}`}>{item.entry_type === "folder"?<Folder size={24}/>:<File size={24}/>}<div>{item.name}<small>{size(item.size)}</small></div></Link></li>)}</ul>{contents.hasNextPage&&<Button onClick={()=>void contents.fetchNextPage()}>Load more</Button>}</>:<Preview entry={preview} publicContentUrl={`${route(entry.path)}?view=inline`} />}</Page></main>;
}
