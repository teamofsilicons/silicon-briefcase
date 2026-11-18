"use client";

/**
 * Settings, on Silicon Accounts' page rules: the page bar, then sections (an h2 over one boxed list or one boxed form):
 * the drive's usage, incoming shares, your own storage and reporting a problem.
 */
import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { CloudUpload, HardDrive, UserRound } from "lucide-react";
import { Alert } from "@teamofsilicons/silicon-ui/alert";
import { Badge } from "@teamofsilicons/silicon-ui/badge";
import { Button } from "@teamofsilicons/silicon-ui/button";
import { Input } from "@teamofsilicons/silicon-ui/input";
import { Progress } from "@teamofsilicons/silicon-ui/progress";
import { Switch } from "@teamofsilicons/silicon-ui/switch";
import { Select } from "@teamofsilicons/silicon-ui/select";
import { Page } from "@/components/foundation/layout/layout";
import { PageBar } from "@/components/foundation/page/page-bar";
import { LinkItem, Row, Section, SectionSkeleton, Tile } from "@/components/foundation/page/rows";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { api } from "@/lib/client/api";
import { notify } from "@/lib/notify";
import { date, message, size, type Drive, type Usage } from "@/lib/briefcase/types";
import styles from "./settings.module.css";

export function Settings({ initialDrives }: { initialDrives: { items: Drive[] } }) {
  const cache = useQueryClient();
  const [drive, setDrive] = useState(initialDrives.items[0]?.owner.uuid ?? "");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const [report, setReport] = useState("");
  const [bucket, setBucket] = useState({ bucket_name: "", region: "us-east-1", role_arn: "", prefix: "briefcase", aws_account_id: "", encryption_mode: "sse_s3", kms_key_arn: "" });
  const chosen = initialDrives.items.find(item => item.owner.uuid === drive);
  const usage = useQuery({ queryKey: ["usage", drive], queryFn: () => api.get<Usage>("/v1/usage", { query: { drive } }), enabled: !!drive });
  const shareSettingsPath = `/v1/accounts/${encodeURIComponent(drive)}/incoming-shares`;
  const shareSettingsKey = ["incoming-shares", drive];
  const incoming = useQuery({
    queryKey: shareSettingsKey,
    queryFn: ({ signal }) => api.get<{ allow_from_anyone: boolean }>(shareSettingsPath, { signal }),
    enabled: !!drive,
  });
  async function act(work: () => Promise<unknown>, success: string) {
    setBusy(true);
    setFailure("");
    try { await work(); notify.success(success); }
    catch (error) { setFailure(message(error)); }
    finally { setBusy(false); }
  }
  async function saveIncomingShares(allowFromAnyone: boolean) {
    await act(async () => {
      await cache.cancelQueries({ queryKey: shareSettingsKey });
      const saved = await api.put<{ allow_from_anyone: boolean }>(shareSettingsPath, { allow_from_anyone: allowFromAnyone });
      cache.setQueryData(shareSettingsKey, saved);
    }, "Incoming share settings saved");
  }
  const meters = usage.data ? [
    { key: "storage", label: "Storage", icon: HardDrive, data: usage.data.storage, detail: `${size(usage.data.storage.used_bytes)} of ${size(usage.data.storage.limit_bytes)}` },
    { key: "uploads", label: "Uploads today", icon: CloudUpload, data: usage.data.daily_uploads, detail: `${size(usage.data.daily_uploads.used_bytes)} of ${size(usage.data.daily_uploads.limit_bytes)} · returns ${date(usage.data.daily_uploads.resets_at)}` },
  ] : [];

  return <>
    <PageBar title="Settings" />
    <Page>
      <p className={styles.lead}>A drive that works the way you do.</p>
      {failure ? <Alert tone="danger" title="That did not work" onDismiss={() => setFailure("")}>{failure}</Alert> : null}
      {initialDrives.items.length > 1 ? (
        <div className={styles.drive}>
          <Select label="Drive" disabled={busy} value={drive} onValueChange={setDrive} options={initialDrives.items.map(item => ({ value: item.owner.uuid, label: `${item.owner.display_name || item.owner.id || "My drive"}${item.relation === "own" ? " · yours" : ""}` }))} />
        </div>
      ) : null}

      {usage.error ? <ErrorAlert error={usage.error} /> : null}
      {usage.isPending && drive ? <SectionSkeleton rows={2} title label="Loading usage" /> : meters.length ? (
        <Section id="usage" title="Usage">
          {meters.map(meter => {
            const Icon = meter.icon;
            const percent = meter.data.limit_bytes ? Math.min(100, Math.round((meter.data.used_bytes / meter.data.limit_bytes) * 100)) : 0;
            return (
              <Row key={meter.key} lead={<Tile><Icon size={16} strokeWidth={2} /></Tile>} title={meter.label} detail={meter.detail}>
                <span className={styles.meter} aria-hidden="true"><Progress value={meter.data.used_bytes} max={meter.data.limit_bytes || 1} /></span>
                <Badge size="sm" tone={percent >= 90 ? "warning" : "neutral"} aria-label={`${percent}% used`}>{percent}%</Badge>
              </Row>
            );
          })}
        </Section>
      ) : null}

      {chosen ? (
        <Section id="incoming-shares" title="Incoming shares">
          <Row lead={<Tile><UserRound size={16} strokeWidth={2} /></Tile>}
            title={<span id="incoming-shares-label">Allow shares from anyone</span>}
            detail={<span id="incoming-shares-detail">{chosen.owner.kind === "silicon"
              ? "Turn off to review new senders. Saved contacts, allowed senders, this Silicon’s custodian and other Silicons with the same custodian can share directly."
              : "Turn off to review new senders. Saved contacts, allowed senders and the Silicons this account looks after can share directly."}</span>}>
            {incoming.data ? (
              <Switch aria-labelledby="incoming-shares-label" aria-describedby="incoming-shares-detail"
                checked={incoming.data.allow_from_anyone} disabled={busy || incoming.isError}
                onCheckedChange={value => void saveIncomingShares(value)} />
            ) : incoming.isPending ? <span role="status">Loading…</span> : null}
          </Row>
          {incoming.data?.allow_from_anyone === false ? <LinkItem href={`/share-review?drive=${encodeURIComponent(drive)}`}
            lead={<Tile><UserRound size={16} /></Tile>} title="Review incoming shares" detail="Accept files and folders or allow a sender." /> : null}
        </Section>
      ) : null}
      {incoming.error ? <ErrorAlert error={incoming.error} action={<Button size="sm" variant="ghost" onClick={() => void incoming.refetch()}>Try Again</Button>} /> : null}

      <section className={styles.section} aria-labelledby="storage-title">
        <div className={styles.head}>
          <h2 id="storage-title" className={styles.title}>Bring your own storage</h2>
          <p className={styles.description}>Connect an S3 bucket for future uploads. Existing versions stay where they are. Briefcase verifies read, write and deletion before using the bucket.</p>
        </div>
        <form data-sq="surface" className={styles.box} onSubmit={event => { event.preventDefault(); void act(async () => { const result = await api.put<{ status: string; failure_reason: string | null }>("/v1/storage/configuration", { ...bucket, drive, kms_key_arn: bucket.encryption_mode === "sse_kms" ? bucket.kms_key_arn : null }); if (result.status !== "configured") throw new Error(result.failure_reason || "The bucket did not pass verification; the previous storage configuration is still active."); }, "Storage verified and connected"); }}>
          <div className={styles.fields}>
            {([{ key: "bucket_name", label: "Bucket name" }, { key: "region", label: "AWS region" }, { key: "role_arn", label: "Role ARN" }, { key: "prefix", label: "Object prefix" }, { key: "aws_account_id", label: "AWS account ID" }] as const).map(field => <Input key={field.key} label={field.label} required value={bucket[field.key]} onChange={event => setBucket(previous => ({ ...previous, [field.key]: event.target.value }))} />)}
            <Select label="Encryption" value={bucket.encryption_mode} onValueChange={encryption_mode => setBucket(previous => ({ ...previous, encryption_mode }))} options={[{ value: "sse_s3", label: "S3 managed encryption" }, { value: "sse_kms", label: "KMS encryption" }]} />
            {bucket.encryption_mode === "sse_kms" ? <Input label="KMS key ARN" required value={bucket.kms_key_arn} onChange={event => setBucket(previous => ({ ...previous, kms_key_arn: event.target.value }))} /> : null}
          </div>
          <p className={styles.description}>Allow the Briefcase service role in your bucket role’s trust policy, with this drive’s External ID condition. <a href="https://docs.briefcase.teamofsilicons.com" target="_blank" rel="noreferrer">Storage setup instructions</a></p>
          {chosen?.storage_external_id ? <Input label="STS External ID for this drive" readOnly value={chosen.storage_external_id} onFocus={event => event.target.select()} /> : null}
          <div className={styles.end}><Button loading={busy} type="submit" size="sm">Verify and Connect Bucket</Button></div>
        </form>
      </section>

      <section className={styles.section} aria-labelledby="report-title">
        <div className={styles.head}>
          <h2 id="report-title" className={styles.title}>Report a problem</h2>
          <p className={styles.description}>Describe what happened, what you expected, and how to reproduce it.</p>
        </div>
        <form data-sq="surface" className={styles.box} onSubmit={event => { event.preventDefault(); void act(() => api.post("/v1/reports", { message: report }, { idempotencyKey: true }), "Report submitted"); }}>
          <Input label="Problem details" value={report} onChange={event => setReport(event.target.value)} required />
          <div className={styles.end}><Button loading={busy} variant="secondary" size="sm">Send Report</Button></div>
        </form>
      </section>
    </Page>
  </>;
}
