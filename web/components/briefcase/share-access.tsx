"use client";

/**
 * Sharing a file or folder: with whom (people, a team, or the whole organization, as Silicon UI's segmented control),
 * then the recipient, the access and an optional expiry, and the one primary action, as a section: its h2 over one box,
 * like the other sections of a file's Sharing tab. Notices are Silicon UI alerts.
 */
import { useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Alert } from "@teamofsilicons/silicon-ui/alert";
import { Button } from "@teamofsilicons/silicon-ui/button";
import { Input } from "@teamofsilicons/silicon-ui/input";
import { Select } from "@teamofsilicons/silicon-ui/select";
import SegmentedControl from "@teamofsilicons/silicon-ui/segmented-control";
import { api } from "@/lib/client/api";
import type { Person } from "@/lib/briefcase/people";
import { groupShareBody } from "@/lib/briefcase/organizations";
import { RecipientPicker } from "./recipient-picker";
import { ContactEditor } from "./contacts";
import { useOrganizations } from "./organizations-query";
import styles from "./people.module.css";

export function ShareAccess({ endpoint, folder, busy, minutes, setMinutes, act }: { endpoint: string; folder: boolean; busy: boolean; minutes: string; setMinutes: (minutes: string) => void; act: (work: () => Promise<unknown>, success: string) => Promise<void> }) {
  const organizations = useOrganizations();
  const [kind, setKind] = useState("account");
  const [recipient, setRecipient] = useState<Person | null>(null);
  const [editingContact, setEditingContact] = useState<Person | null>(null);
  const cache = useQueryClient();
  const [targetId, setTargetId] = useState("");
  const [rights, setRights] = useState("read");
  const pending = useRef<{ fingerprint: string; key: string } | null>(null);
  const targets = organizations.data?.share_targets.filter(target => target.type === kind) ?? [];
  const target = targets.find(target => target.id === targetId);
  async function share() {
    let body;
    if (kind === "account") {
      if (!recipient) throw new Error("Choose a Carbon or Silicon before sharing.");
      const lifetime = minutes ? Number(minutes) : null;
      if (lifetime !== null && (!Number.isInteger(lifetime) || lifetime < 1 || lifetime > 43200)) throw new Error("Choose a whole number of minutes from 1 to 43,200.");
      body = { principal: { type: recipient.kind, id: recipient.uuid }, access: lifetime ? ["read"] : rights.split(","), inherit: folder, ...(lifetime ? { expires_in_minutes: lifetime } : {}) };
    } else {
      if (!target || organizations.error) throw new Error("Choose an available organization or team after refreshing the list.");
      body = groupShareBody(target, rights.split(","), folder, minutes);
    }
    const fingerprint = JSON.stringify(body);
    if (pending.current?.fingerprint !== fingerprint) pending.current = { fingerprint, key: crypto.randomUUID() };
    const value = await api.post(endpoint + "/invitations", body, { idempotencyKey: pending.current.key });
    pending.current = null;
    await cache.invalidateQueries({ queryKey: ["people"] });
    return value;
  }
  return <section className={styles.shareSection} aria-labelledby="share-access-title">
    <div className={styles.shareHead}>
      <h2 id="share-access-title" className={styles.shareTitle}>Share access</h2>
      <p className={styles.shareText}>Choose people, a particular team, or everyone in your organization.</p>
    </div>
    <form data-sq="surface" className={styles.share} onSubmit={event => { event.preventDefault(); void act(share, "Access shared"); }} aria-labelledby="share-access-title">
    <SegmentedControl size="sm" label="Share with" value={kind} onValueChange={value => { setKind(value); setTargetId(""); }} options={[{ value: "account", label: "People" }, { value: "team", label: "Team" }, { value: "organization", label: "Organization" }]} />
    {kind === "account" ? <RecipientPicker value={recipient} onChange={setRecipient} disabled={busy} onSaveContact={setEditingContact} />
      : organizations.error ? <Alert tone="danger" title="Your organizations could not be loaded"><Button type="button" variant="secondary" size="sm" onClick={() => void organizations.refetch()}>Try Again</Button></Alert>
      : organizations.isPending ? <p role="status" className={styles.shareText}>Loading available teams…</p>
      : targets.length ? <>
        <Select label={kind === "team" ? "Team" : "Organization"} placeholder={`Choose ${kind === "team" ? "a team" : "an organization"}`} value={targetId} onValueChange={setTargetId} options={targets.map(target => ({ value: target.id, label: kind === "team" ? `${target.name} · ${target.org_id}` : target.name }))} />
        <p className={styles.shareText}>{kind === "team" ? "Current and future members of this team receive access. Leaving the team ends access from this share." : "Current and future members of this organization receive access."}</p>
      </>
      : <Alert tone="info" title={kind === "team" ? "No teams are available to this account" : "This account has no organization membership"}>Open Organizations or switch to your work account to manage team sharing.</Alert>}
    <div className={styles.fields}>
      <Select label="Access" value={rights} onValueChange={setRights} disabled={!!minutes} options={[{ value: "read", label: "Can view and download" }, { value: "read,update", label: "Can view and edit" }, ...(folder ? [{ value: "read,write", label: "Can view and add files" }, { value: "read,write,update", label: "Can view, add, and edit" }] : [])]} />
      <Input label="Expires after (minutes)" type="number" min={1} max={43200} step={1} placeholder="No expiry" value={minutes} onChange={event => setMinutes(event.target.value)} description={minutes ? "Expiring shares allow viewing and downloading only, for up to 30 days." : undefined} />
    </div>
    <div className={styles.shareActions}>
      <Button type="submit" loading={busy} disabled={kind === "account" ? !recipient : (!target || !!organizations.error)}>Share {kind === "team" ? "with Team" : kind === "organization" ? "with Organization" : "Access"}</Button>
    </div>
    </form>
    {editingContact ? <ContactEditor initial={editingContact} onClose={() => setEditingContact(null)} onSaved={setRecipient} /> : null}
  </section>;
}
