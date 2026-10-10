"use client";
import { useQuery } from "@tanstack/react-query";
import Link from "next/link";
import { Button } from "@/components/silicon-ui/button/button";
import { EmptyState } from "@/components/silicon-ui/empty-state/empty-state";
import { Page, PageHeader } from "@/components/foundation/layout/layout";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { api } from "@/lib/client/api";
import { notify } from "@/lib/notify";
import { date, type Notification } from "@/lib/briefcase/types";
import styles from "./briefcase.module.css";
export function Inbox() {
  const inbox = useQuery({ queryKey:["inbox"],queryFn:() => api.get<{items:Notification[];unread_count:number}>("/v1/notifications") });
  return <Page width="reading"><PageHeader title="Notifications" description={inbox.data?.unread_count ? `${inbox.data.unread_count} unread updates` : "Sharing updates, all in one place."} actions={<Button variant="secondary" onClick={() => void api.post("/v1/notifications/read",{}).then(() => inbox.refetch()).catch(notify.error)}>Mark all read</Button>} />{inbox.error && <ErrorAlert error={inbox.error} />}{inbox.isPending ? <p role="status">Loading notifications…</p> : inbox.data?.items.length === 0 ? <EmptyState title="You’re all caught up" description="New shares and changes to your access appear here." /> : <ul className={styles.list}>{inbox.data?.items.map(item => <li key={item.id}><div><strong>{item.actor?.id || "Briefcase"} · {item.kind.replaceAll("_"," ")}{!item.read && " · New"}</strong>{item.subject && <p><Link href={`/files?entry=${encodeURIComponent(item.subject.entry_id)}`}>{item.subject.name}</Link></p>}<small>{date(item.created_at)}</small></div></li>)}</ul>}</Page>;
}
