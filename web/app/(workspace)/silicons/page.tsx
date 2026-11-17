/** My Silicons: the drives of the Silicons this account looks after, as one boxed list (Accounts' Silicons page). */
import { Bot } from "lucide-react";
import { EmptyState } from "@teamofsilicons/silicon-ui/empty-state";
import { Page } from "@/components/foundation/layout/layout";
import { PageBar } from "@/components/foundation/page/page-bar";
import { LinkItem, Section, Tile } from "@/components/foundation/page/rows";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { tryApiFetch } from "@/lib/server/rsc";
import type { Drive } from "@/lib/briefcase/types";
import styles from "@/components/briefcase/inbox.module.css";

export default async function SiliconsPage() {
  const result = await tryApiFetch<{ items: Drive[] }>("/v1/drives");
  const drives = result.data?.items.filter(item => item.relation === "custodial") ?? [];
  return <>
    <PageBar title="My Silicons" />
    <Page>
      <p className={styles.lead}>Look after their files, while acting as yourself.</p>
      {result.error ? <ErrorAlert error={result.error} />
        : drives.length ? (
          <Section id="silicons" title="Silicons you look after" count={drives.length}>
            {drives.map(drive => <LinkItem key={drive.owner.uuid} href={`/files?drive=${encodeURIComponent(drive.owner.uuid)}`} lead={<Tile><Bot size={16} strokeWidth={2} /></Tile>} title={drive.owner.display_name || drive.owner.id || "Silicon"} detail={`${drive.owner.id ?? drive.owner.uuid} · Their files`} />)}
          </Section>
        ) : <div className={styles.empty}><EmptyState title="Their files will be here" description="When a Silicon you look after uses Briefcase, you can open and manage its drive here." icon={<Bot size={20} strokeWidth={2} />} /></div>}
    </Page>
  </>;
}
