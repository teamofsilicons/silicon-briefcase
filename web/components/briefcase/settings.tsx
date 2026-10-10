"use client";
import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "@/components/arc/button/button";
import { Input } from "@/components/arc/input/input";
import { Select } from "@/components/arc/select/select";
import { Page, PageHeader, Section } from "@/components/foundation/layout/layout";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { api } from "@/lib/client/api";
import { notify } from "@/lib/notify";
import { date, message, size, type Account, type Drive, type Usage } from "@/lib/briefcase/types";
import { TelemetryPreference } from "./telemetry";
import styles from "./briefcase.module.css";
export function Settings({ initialDrives }: { initialDrives: { items: Drive[] } }) {
  const cache = useQueryClient();
  const [drive, setDrive] = useState(initialDrives.items[0]?.owner.uuid ?? "");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const [allowed, setAllowed] = useState("");
  const [report, setReport] = useState("");
  const [bucket, setBucket] = useState({bucket_name:"",region:"us-east-1",role_arn:"",prefix:"briefcase",aws_account_id:"",encryption_mode:"sse_s3",kms_key_arn:""});
  const chosen = initialDrives.items.find(item => item.owner.uuid === drive);
  const usage = useQuery({ queryKey: ["usage", drive], queryFn: () => api.get<Usage>("/v1/usage", { query: { drive } }), enabled: !!drive });
  const allowances = useQuery({ queryKey: ["allowances",drive], queryFn: () => api.get<{ items: { account: Account; allowed_at: string }[] }>(`/v1/silicons/${encodeURIComponent(drive)}/allowed-accounts`), enabled: chosen?.owner.kind === "silicon" });
  async function act(work: () => Promise<unknown>, success: string) {
    setBusy(true); setFailure(""); try { await work(); notify.success(success); await cache.invalidateQueries({ queryKey: ["allowances",drive] }); } catch(error) { setFailure(message(error)); } finally { setBusy(false); }
  }
  return <Page width="reading"><PageHeader title="Settings" description="A drive that works the way you do." />
    {failure && <p role="alert" data-sq="surface" className={styles.error}>{failure}</p>}
    <div className={styles.toolbar}><Select label="Drive" value={drive} onValueChange={setDrive} options={initialDrives.items.map(item => ({value:item.owner.uuid,label:`${item.owner.display_name || item.owner.id || "My drive"}${item.relation === "own" ? " · yours" : ""}`}))} /></div>
    {usage.error && <ErrorAlert error={usage.error} />}
    {usage.data && <div className={styles.stats}>{[{label:"Storage",data:usage.data.storage},{label:"Uploads today",data:usage.data.daily_uploads}].map(item => <div key={item.label} data-sq="surface" className={styles.stat}><span className={styles.muted}>{item.label}</span><strong>{size(item.data.used_bytes)}</strong><p className={styles.muted}>of {size(item.data.limit_bytes)}</p><progress aria-label={item.label} value={item.data.used_bytes} max={item.data.limit_bytes} /></div>)}</div>}
    {usage.data && <p className={styles.muted}>Your daily upload allowance returns {date(usage.data.daily_uploads.resets_at)}.</p>}
    {chosen?.owner.kind === "silicon" && <Section title="Who can share with this Silicon" description="Its custodian and the other Silicons they look after are already allowed. Add other accounts explicitly.">
      {allowances.error && <ErrorAlert error={allowances.error} />}
      <form className={styles.toolbar} onSubmit={event => { event.preventDefault(); void act(() => api.put(`/v1/silicons/${encodeURIComponent(drive)}/allowed-accounts/${encodeURIComponent(allowed.trim())}`), "Account allowed to share"); }}><Input label="Account id" placeholder="c:ada or si:scout" value={allowed} onChange={event => setAllowed(event.target.value)} required /><Button loading={busy}>Allow sharing</Button></form>
      <ul className={styles.list}>{allowances.data?.items.map(item => <li key={item.account.uuid}><div><strong>{item.account.display_name || item.account.id}</strong><small>{item.account.id}</small></div><Button variant="ghost" size="sm" loading={busy} onClick={() => void act(() => api.delete(`/v1/silicons/${encodeURIComponent(drive)}/allowed-accounts/${encodeURIComponent(item.account.uuid)}`), "Sharing allowance removed")}>Remove</Button></li>)}</ul>
    </Section>}
    <Section title="Bring your own storage" description="Connect an S3 bucket for future uploads. Existing versions stay where they are. Briefcase verifies read, write and deletion before using the bucket.">
      <form className={styles.form} onSubmit={event => { event.preventDefault(); void act(async () => { const result = await api.put<{status:string;failure_reason:string|null}>("/v1/storage/configuration", { ...bucket, drive, kms_key_arn: bucket.encryption_mode === "sse_kms" ? bucket.kms_key_arn : null }); if(result.status !== "configured") throw new Error(result.failure_reason || "The bucket did not pass verification; the previous storage configuration is still active."); }, "Storage verified and connected"); }}>
        <div className={styles.detailGrid}>{([{key:"bucket_name",label:"Bucket name"},{key:"region",label:"AWS region"},{key:"role_arn",label:"Role ARN"},{key:"prefix",label:"Object prefix"},{key:"aws_account_id",label:"AWS account id"}] as const).map(field => <Input key={field.key} label={field.label} required value={bucket[field.key]} onChange={event => setBucket(previous => ({...previous,[field.key]:event.target.value}))} />)}<Select label="Encryption" value={bucket.encryption_mode} onValueChange={encryption_mode => setBucket(previous => ({...previous,encryption_mode}))} options={[{value:"sse_s3",label:"S3 managed encryption"},{value:"sse_kms",label:"KMS encryption"}]} />{bucket.encryption_mode === "sse_kms" && <Input label="KMS key ARN" required value={bucket.kms_key_arn} onChange={event => setBucket(previous => ({...previous,kms_key_arn:event.target.value}))} />}</div>
        <p className={styles.muted}>Allow the Briefcase service role in your bucket role’s trust policy, with this drive’s External ID condition. <a href="https://docs.briefcase.teamofsilicons.com" target="_blank" rel="noreferrer">Storage setup instructions</a></p>{chosen?.storage_external_id && <Input label="STS External ID for this drive" readOnly value={chosen.storage_external_id} onFocus={event => event.target.select()} />}<Button loading={busy} type="submit">Verify and connect bucket</Button>
      </form>
    </Section>
    <Section title="Diagnostics" description="Applies to this browser. File contents, names, credentials and typed text are excluded."><TelemetryPreference /></Section>
    <Section title="Report a problem" description="Describe what happened, what you expected, and how to reproduce it."><form className={styles.form} onSubmit={event => { event.preventDefault(); void act(() => api.post("/v1/reports", {message:report}, { idempotencyKey:true }), "Report submitted"); }}><Input label="Problem details" value={report} onChange={event => setReport(event.target.value)} required /><Button loading={busy} variant="secondary">Send report</Button></form></Section>
  </Page>;
}
