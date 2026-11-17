"use client";

/**
 * Notifications, as Silicon Accounts' Activity reads: the page bar with Mark All Read, then a section per day, each a
 * boxed list of updates (who did what, the file it was about, and when), the newest first.
 */
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Bell, Share2, UserRoundCog } from "lucide-react";
import { Badge } from "@teamofsilicons/silicon-ui/badge";
import { Button } from "@teamofsilicons/silicon-ui/button";
import { EmptyState } from "@teamofsilicons/silicon-ui/empty-state";
import { Page } from "@/components/foundation/layout/layout";
import { PageBar } from "@/components/foundation/page/page-bar";
import { LinkItem, Row, Section, SectionSkeleton, Tile } from "@/components/foundation/page/rows";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { api } from "@/lib/client/api";
import { notify } from "@/lib/notify";
import { date, type Notification } from "@/lib/briefcase/types";
import styles from "./inbox.module.css";

/** "Today", "Yesterday", or the day itself. */
function dayOf(value: string) {
  const day = new Date(value); const today = new Date();
  const start = (when: Date) => new Date(when.getFullYear(), when.getMonth(), when.getDate()).getTime();
  const days = Math.round((start(today) - start(day)) / 86_400_000);
  return days === 0 ? "Today" : days === 1 ? "Yesterday" : day.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" });
}

const sentence = (item: Notification) => {
  const what = item.kind.replaceAll("_", " ");
  return `${item.actor?.id || "Briefcase"} · ${what[0].toUpperCase()}${what.slice(1)}`;
};

export function Inbox() {
  const cache = useQueryClient();
  const inbox = useQuery({ queryKey: ["inbox"], queryFn: () => api.get<{ items: Notification[]; unread_count: number }>("/v1/notifications") });
  const unread = inbox.data?.unread_count ?? 0;
  const days = new Map<string, Notification[]>();
  for (const item of inbox.data?.items ?? []) {
    const key = dayOf(item.created_at);
    days.set(key, [...(days.get(key) ?? []), item]);
  }
  const markAllRead = () => void api.post("/v1/notifications/read", {}).then(() => Promise.all([cache.invalidateQueries({ queryKey: ["inbox"] }), cache.invalidateQueries({ queryKey: ["notifications-unread"] })])).catch(notify.error);
  return <>
    <PageBar title="Notifications" actions={<Button variant="secondary" size="sm" disabled={!unread} onClick={markAllRead}>Mark All Read</Button>} />
    <Page>
      <p className={styles.lead}>{unread ? `${unread} unread ${unread === 1 ? "update" : "updates"}.` : "Sharing updates, all in one place."}</p>
      {inbox.error ? <ErrorAlert error={inbox.error} /> : null}
      {inbox.isPending ? <SectionSkeleton rows={3} title label="Loading notifications" />
        : inbox.data?.items.length === 0 ? <div className={styles.empty}><EmptyState title="You’re all caught up" description="New shares and changes to your access appear here." icon={<Bell size={20} strokeWidth={2} />} /></div>
        : [...days].map(([day, items]) => (
          <Section key={day} id={`day-${day.replaceAll(/\W+/g, "-").toLowerCase()}`} title={day} count={items.length}>
            {items.map(item => {
              const Icon = item.kind.includes("share") ? Share2 : UserRoundCog;
              const lead = <Tile><Icon size={16} strokeWidth={2} /></Tile>;
              const end = <span className={styles.end}>{!item.read ? <Badge size="sm" tone="info">New</Badge> : null}<span className={styles.time}>{new Date(item.created_at).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}</span></span>;
              return item.subject
                ? <LinkItem key={item.id} href={`/files?entry=${encodeURIComponent(item.subject.entry_id)}`} lead={lead} title={item.subject.name} detail={sentence(item)} end={end} label={`${item.subject.name}, ${sentence(item)}, ${date(item.created_at)}`} />
                : <Row key={item.id} lead={lead} title={sentence(item)} detail={date(item.created_at)}>{end}</Row>;
            })}
          </Section>
        ))}
    </Page>
  </>;
}
