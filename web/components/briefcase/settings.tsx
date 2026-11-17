"use client";

/**
 * Settings, on Silicon Accounts' page rules: the page bar, then sections (an h2 over one boxed list or one boxed form):
 * the drive's usage, who may share with a Silicon, your own storage and reporting a problem.
 */
import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { CloudUpload, HardDrive, UserRound } from "lucide-react";
import { Alert } from "@teamofsilicons/silicon-ui/alert";
import { Badge } from "@teamofsilicons/silicon-ui/badge";
import { Button } from "@teamofsilicons/silicon-ui/button";
import { Input } from "@teamofsilicons/silicon-ui/input";
import { Progress } from "@teamofsilicons/silicon-ui/progress";
import { Select } from "@teamofsilicons/silicon-ui/select";
import { Page } from "@/components/foundation/layout/layout";
import { PageBar } from "@/components/foundation/page/page-bar";
import { Row, Section, Tile } from "@/components/foundation/page/rows";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { api } from "@/lib/client/api";
import { notify } from "@/lib/notify";
import { date, message, size, type Account, type Drive, type Usage } from "@/lib/briefcase/types";
import styles from "./settings.module.css";

export function Settings({ initialDrives }: { initialDrives: { items: Drive[] } }) {
  const cache = useQueryClient();
  const [drive, setDrive] = useState(initialDrives.items[0]?.owner.uuid ?? "");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const [allowed, setAllowed] = useState("");
  const [report, setReport] = useState("");
  const [bucket, setBucket] = useState({ bucket_name: "", region: "us-east-1", role_arn: "", prefix: "briefcase", aws_account_id: "", encryption_mode: "sse_s3", kms_key_arn: "" });
  const chosen = initialDrives.items.find(item => item.owner.uuid === drive);
  const usage = useQuery({ queryKey: ["usage", drive], queryFn: () => api.get<Usage>("/v1/usage", { query: { drive } }), enabled: !!drive });
  const allowances = useQuery({ queryKey: ["allowances", drive], queryFn: () => api.get<{ items: { account: Account; allowed_at: string }[] }>(`/v1/silicons/${encodeURIComponent(drive)}/allowed-accounts`), enabled: chosen?.owner.kind === "silicon" });
  async function act(work: () => Promise<unknown>, success: string) {
    setBusy(true); setFailure(""); try { await work(); notify.success(success); await cache.invalidateQueries({ queryKey: ["allowances", drive] }); } catch (error) { setFailure(message(error)); } finally { setBusy(false); }
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
          <Select label="Drive" value={drive} onValueChange={setDrive} options={initialDrives.items.map(item => ({ value: item.owner.uuid, label: `${item.owner.display_name || item.owner.id || "My drive"}${item.relation === "own" ? " · yours" : ""}` }))} />
        </div>
      ) : null}

      {usage.error ? <ErrorAlert error={usage.error} /> : null}
      {meters.length ? (
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

      {chosen?.owner.kind === "silicon" ? (
        <Section id="allowed" title="Who can share with this Silicon" description="Its custodian and the other Silicons they look after are already allowed. Add other accounts explicitly.">
          {allowances.data?.items.map(item => (
            <Row key={item.account.uuid} lead={<Tile><UserRound size={16} strokeWidth={2} /></Tile>} title={item.account.display_name || item.account.id} detail={item.account.id}>
              <Button variant="ghost" size="sm" loading={busy} onClick={() => void act(() => api.delete(`/v1/silicons/${encodeURIComponent(drive)}/allowed-accounts/${encodeURIComponent(item.account.uuid)}`), "Sharing allowance removed")}>Remove</Button>
            </Row>
          ))}
          <li className={styles.formRow}>
            <form className={styles.inlineForm} onSubmit={event => { event.preventDefault(); void act(() => api.put(`/v1/silicons/${encodeURIComponent(drive)}/allowed-accounts/${encodeURIComponent(allowed.trim())}`), "Account allowed to share"); }}>
              <Input label="Account ID" placeholder="c:ada or si:scout" value={allowed} onChange={event => setAllowed(event.target.value)} required />
              <Button size="sm" loading={busy}>Allow Sharing</Button>
            </form>
          </li>
        </Section>
      ) : null}
      {chosen?.owner.kind === "silicon" && allowances.error ? <ErrorAlert error={allowances.error} /> : null}

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
