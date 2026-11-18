"use client";

import { useState } from "react";
import { useInfiniteQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { File, Folder, UserRound } from "lucide-react";
import { Button } from "@teamofsilicons/silicon-ui/button";
import { Input } from "@teamofsilicons/silicon-ui/input";
import { Select } from "@teamofsilicons/silicon-ui/select";
import { Page } from "@/components/foundation/layout/layout";
import { PageBar } from "@/components/foundation/page/page-bar";
import { Row, Section, SectionSkeleton, Tile } from "@/components/foundation/page/rows";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { type Account, type Drive, accessLabel, date } from "@/lib/briefcase/types";
import { api } from "@/lib/client/api";
import { notify } from "@/lib/notify";
import styles from "./settings.module.css";

type Offer = { id: string; name: string; type: "file" | "folder"; sender: Account; access: string[]; inherit: boolean; expires_at: string | null };
type ReviewPage = { items: Offer[]; allowed_senders: Account[]; next_cursor: string | null };
type Action = { decision: "accept" | "allow_sender" | "dismiss" | "remove_sender" | "add_sender"; id: string };
const resultMessage = { add_sender: "Sender allowed and pending shares accepted", accept: "Share accepted", allow_sender: "Sender allowed and pending shares accepted", dismiss: "Share dismissed", remove_sender: "Sender removed" };

export function ShareReview({ drives, initialDrive }: { drives: Drive[]; initialDrive?: string }) {
  const [drive, setDrive] = useState(drives.find(d => d.owner.uuid === initialDrive)?.owner.uuid ?? drives[0]?.owner.uuid ?? "");
  return <>
    <PageBar title="Share review" />
    <Page>
      {drives.length > 1 ? <div className={styles.drive}><Select label="Drive" value={drive} onValueChange={setDrive}
        options={drives.map(({ owner }) => ({ value: owner.uuid, label: owner.display_name || owner.id || owner.uuid }))} /></div> : null}
      {drive ? <ReviewList key={drive} drive={drive} /> : null}
    </Page>
  </>;
}

function ReviewList({ drive }: { drive: string }) {
  const cache = useQueryClient();
  const [senderId, setSenderId] = useState("");
  const base = `/v1/accounts/${encodeURIComponent(drive)}/share-review`;
  const query = useInfiniteQuery({
    queryKey: ["share-review", drive], initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) => api.get<ReviewPage>(base, { query: { cursor: pageParam }, signal }),
    getNextPageParam: last => last.next_cursor ?? undefined, refetchInterval: 30000,
  });
  const action = useMutation({
    mutationFn: ({ decision, id }: Action) => decision === "remove_sender"
      ? api.delete(`${base}/senders/${encodeURIComponent(id)}`)
      : decision === "add_sender" ? api.put(`${base}/senders/${encodeURIComponent(id)}`, {})
      : api.post(`${base}/${encodeURIComponent(id)}`, { decision }),
    onError: () => cache.invalidateQueries({ queryKey: ["share-review", drive] }),
    onSuccess: async (_, { decision }) => {
      notify.success(resultMessage[decision]);
      if (decision === "add_sender") setSenderId("");
      await Promise.all(["share-review", "shared", "inbox", "notifications-unread"].map(key => cache.invalidateQueries({ queryKey: [key] })));
    },
  });
  const offers = query.data?.pages.flatMap(page => page.items) ?? [];
  const allowed = query.data?.pages[0]?.allowed_senders ?? [];
  return <>
    {query.error ? <ErrorAlert error={query.error} action={<Button size="sm" variant="ghost" onClick={() => void query.refetch()}>Try Again</Button>} /> : null}
    {action.error ? <ErrorAlert error={action.error} /> : null}
    {query.isPending ? <SectionSkeleton title rows={2} label="Loading shares for review" /> : query.data ? <>
      <Section id="pending-shares" title="Pending shares" description="Accept one item, or allow a sender to accept all their pending and future shares.">
        {offers.map(offer => {
          const Icon = offer.type === "folder" ? Folder : File;
          const sender = offer.sender.id || offer.sender.uuid;
          const detail = [sender, accessLabel(offer.access), offer.type === "folder" && offer.inherit ? "Includes contents" : null, offer.expires_at ? `Until ${date(offer.expires_at)}` : null].filter(Boolean).join(" · ");
          return <Row wide key={offer.id} lead={<Tile><Icon size={16} /></Tile>} title={offer.name} detail={detail}>
            <Button size="sm" disabled={action.isPending} onClick={() => action.mutate({ id: offer.id, decision: "accept" })}>Accept {offer.type === "folder" ? "Folder" : "File"}</Button>
            <Button size="sm" variant="secondary" disabled={action.isPending} onClick={() => action.mutate({ id: offer.id, decision: "allow_sender" })}>Allow Sender</Button>
            <Button size="sm" variant="ghost" disabled={action.isPending} onClick={() => action.mutate({ id: offer.id, decision: "dismiss" })}>Dismiss</Button>
          </Row>;
        })}
        {!offers.length ? <Row lead={null} title="No shares waiting for review" /> : null}
        {query.hasNextPage ? <Row lead={null} title={<Button size="sm" variant="ghost" loading={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()}>Show More</Button>} /> : null}
      </Section>
      <Section id="allowed-senders" title="Allowed senders">
        <Row wide lead={<Tile><UserRound size={16} /></Tile>} title="Add a Carbon or Silicon" detail="Accept this account’s pending and future shares.">
          <form className={styles.senderForm} onSubmit={event => { event.preventDefault(); if (senderId.trim()) action.mutate({ decision: "add_sender", id: senderId.trim() }); }}>
            <Input label="Sender account ID" placeholder="c:name or si:name" value={senderId} onChange={event => setSenderId(event.target.value)} required />
            <Button type="submit" size="sm" variant="secondary" disabled={action.isPending || !senderId.trim()}>Add Sender</Button>
          </form>
        </Row>
        {allowed.map(sender => <Row key={sender.uuid} lead={<Tile><UserRound size={16} /></Tile>} title={sender.display_name || sender.id || sender.uuid} detail={sender.id || sender.uuid}>
          <Button size="sm" variant="ghost" disabled={action.isPending} onClick={() => action.mutate({ id: sender.uuid, decision: "remove_sender" })}>Remove</Button>
        </Row>)}
      </Section>
    </> : null}
  </>;
}
