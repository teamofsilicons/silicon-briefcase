/* eslint-disable @next/next/no-img-element -- File previews stream from the authenticated BFF without a shared image cache. */
"use client";
import { useQuery } from "@tanstack/react-query";
import { File, Folder } from "lucide-react";
import { api } from "@/lib/client/api";
import { contentUrl, entryUrl, size, type Entry } from "@/lib/briefcase/types";
import { officePreview, sandboxDocument } from "@/lib/briefcase/office-preview";
import { highlight } from "@/lib/highlight";
import styles from "./briefcase.module.css";

const TEXT_LIMIT = 256 * 1024;
const OFFICE_LIMIT = 10 * 1024 * 1024;
/** A bounded reader also handles a server that ignores Range. */
async function bounded(url: string, limit: number, signal: AbortSignal) {
  const response = await fetch(url, { headers: { Range: `bytes=0-${limit - 1}` }, signal });
  if (!response.ok) throw new Error(`The preview could not be read (${response.status}).`);
  const reader = response.body?.getReader(); if (!reader) return new Uint8Array();
  const chunks: Uint8Array[] = []; let length = 0;
  try { while (length < limit) { const { done, value } = await reader.read(); if (done) break; const chunk = value.slice(0,limit - length); chunks.push(chunk); length += chunk.length; } }
  finally { await reader.cancel(); }
  const output = new Uint8Array(length); let offset = 0; for (const chunk of chunks) { output.set(chunk, offset); offset += chunk.length; }
  return output;
}
export function Preview({ entry, publicContentUrl }: { entry: Entry; publicContentUrl?: string }) {
  const url = publicContentUrl || contentUrl(entry.id);
  const extension = entry.name.split(".").pop()?.toLowerCase() || "";
  const office = ["docx","xlsx","xlsm","pptx","odt","ods","odp"].includes(extension);
  const text = entry.render === "code" || ["txt","md","markdown","csv","tsv","rst","tex","rtf","json","xml","log"].includes(extension);
  const preview = useQuery({ queryKey: ["preview", entry.id, entry.updated_at, publicContentUrl], enabled: entry.type === "file" && (office || text), queryFn: async ({ signal }) => {
    if (office && (entry.size ?? 0) > OFFICE_LIMIT) throw new Error("This document is too large to preview. Download it to open the complete file.");
    const bytes = await bounded(url, office ? OFFICE_LIMIT : TEXT_LIMIT, signal);
    return office ? { html: sandboxDocument(officePreview(bytes,extension)), text: "" } : { html: "", text: new TextDecoder().decode(bytes) };
  } });
  const archive = useQuery({ queryKey: ["archive", entry.id], enabled: entry.render === "archive" && !publicContentUrl, queryFn: () => api.get<{ entries: { path: string; size: number | null; kind: string }[]; truncated: boolean }>(entryUrl(entry.id,"/archive")) });
  if (entry.type === "folder") return <div data-sq="surface" className={styles.preview}><div className={styles.form}><Folder size={52} /><p>Open this folder to browse its contents.</p></div></div>;
  return <><div data-sq="surface" className={styles.preview}>
    {entry.render === "image" ? <img src={url} alt={entry.name} /> : entry.render === "video" ? <video src={url} controls preload="metadata" /> : entry.render === "audio" ? <audio src={url} controls preload="metadata" /> : extension === "pdf" ? <iframe title={`Preview of ${entry.name}`} src={url} sandbox="allow-same-origin" /> : office ? preview.data ? <iframe title={`Preview of ${entry.name}`} srcDoc={preview.data.html} sandbox="" /> : <p role="status">{preview.error?.message || "Preparing document…"}</p> : text ? preview.data ? <pre><code>{highlight(preview.data.text, extension).map((token,index) => <span key={index} style={{ color: token.k === "comment" ? "var(--text-muted)" : token.k === "string" ? "var(--success)" : token.k === "keyword" ? "var(--primary)" : undefined }}>{token.v}</span>)}</code></pre> : <p role="status">{preview.error?.message || "Reading preview…"}</p> : entry.render === "archive" && !publicContentUrl ? archive.data ? <table className={styles.table}><thead><tr><th>Archive contents</th><th>Size</th></tr></thead><tbody>{archive.data.entries.map((item,index) => <tr key={index}><td>{item.path}</td><td>{size(item.size)}</td></tr>)}</tbody></table> : <p>{archive.error?.message || "Reading archive…"}</p> : <div className={styles.form}><File size={48} /><p>Unsupported file type. Download it to open in another app.</p></div>}
  </div>{office && preview.data && <p className={styles.muted}>Text and table preview. Download the original to see its complete formatting, charts and embedded media.</p>}{text && (entry.size ?? 0) > TEXT_LIMIT && <p className={styles.muted}>Showing the first 256 KiB. Download the file for the full contents.</p>}{archive.data?.truncated && <p className={styles.muted}>This archive has more entries than the preview limit.</p>}</>;
}
