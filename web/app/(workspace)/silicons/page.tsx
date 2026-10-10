import Link from "next/link";
import { Bot } from "lucide-react";
import { Page, PageHeader } from "@/components/foundation/layout/layout";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { EmptyState } from "@/components/arc/empty-state/empty-state";
import { tryApiFetch } from "@/lib/server/rsc";
import type { Drive } from "@/lib/briefcase/types";
import styles from "@/components/briefcase/briefcase.module.css";
export default async function SiliconsPage() {
 const result = await tryApiFetch<{ items: Drive[] }>("/v1/drives"); const drives = result.data?.items.filter(item => item.relation === "custodial") ?? [];
 return <Page><PageHeader title="My Silicons" description="Look after their files, while acting as yourself." />{result.error ? <ErrorAlert error={result.error} /> : drives.length ? <div className={styles.grid}>{drives.map(drive => <Link className={styles.tile} data-sq="surface" key={drive.owner.uuid} href={`/files?drive=${encodeURIComponent(drive.owner.uuid)}`}><span className={styles.fileIcon}><Bot size={26} /></span><strong>{drive.owner.display_name || drive.owner.id}</strong><small>{drive.owner.id}</small></Link>)}</div> : <EmptyState title="Their files will be here" description="When a Silicon you look after uses Briefcase, you can open and manage its drive here." />}</Page>;
}
