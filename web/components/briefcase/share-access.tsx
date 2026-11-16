"use client";
import { useRef, useState } from "react";
import { Building2, UserRound, UsersRound } from "lucide-react";
import { Button } from "@/components/silicon-ui/button/button";
import { Input } from "@/components/silicon-ui/input/input";
import { Select } from "@/components/silicon-ui/select/select";
import { api } from "@/lib/client/api";
import { parseAccountId } from "@/lib/ids";
import { groupShareBody } from "@/lib/briefcase/organizations";
import { useOrganizations } from "./organizations-query";
import styles from "./organizations.module.css";

export function ShareAccess({ endpoint, folder, busy, minutes, setMinutes, act }: { endpoint: string; folder: boolean; busy: boolean; minutes: string; setMinutes: (minutes: string) => void; act: (work: () => Promise<unknown>, success: string) => Promise<void> }) {
  const organizations = useOrganizations();
  const [kind, setKind] = useState("account");
  const [recipient, setRecipient] = useState("");
  const [targetId, setTargetId] = useState("");
  const [rights, setRights] = useState("read");
  const pending = useRef<{ fingerprint: string; key: string } | null>(null);
  const targets = organizations.data?.share_targets.filter(target => target.type === kind) ?? [];
  const target = targets.find(target => target.id === targetId);
  async function share() {
    let body;
    if (kind === "account") {
      const checked = parseAccountId(recipient);
      if (!checked.ok) throw new Error(checked.message);
      const lifetime = minutes ? Number(minutes) : null;
      if (lifetime !== null && (!Number.isInteger(lifetime) || lifetime < 1 || lifetime > 43200)) throw new Error("Choose a whole number of minutes from 1 to 43,200.");
      body = { principal: { type: checked.value.kind, id: checked.value.id }, access: lifetime ? ["read"] : rights.split(","), inherit: folder, ...(lifetime ? { expires_in_minutes: lifetime } : {}) };
    } else {
      if (!target || organizations.error) throw new Error("Choose an available organization or team after refreshing the list.");
      body = groupShareBody(target, rights.split(","), folder, minutes);
    }
    const fingerprint = JSON.stringify(body);
    if (pending.current?.fingerprint !== fingerprint) pending.current = { fingerprint, key: crypto.randomUUID() };
    const value = await api.post(endpoint + "/invitations", body, { idempotencyKey: pending.current.key });
    pending.current = null;
    return value;
  }
  return <form className={styles.form} onSubmit={event => { event.preventDefault(); void act(share, "Access shared"); }}>
    <div><h3>Share access</h3><p className={styles.muted}>Choose people, a particular team, or everyone in your organization.</p></div>
    <div className={styles.shareKinds} aria-label="Share with">{[{ id: "account", label: "People", icon: UserRound }, { id: "team", label: "Team", icon: UsersRound }, { id: "organization", label: "Organization", icon: Building2 }].map(item => <button key={item.id} type="button" aria-pressed={kind === item.id} onClick={() => { setKind(item.id); setTargetId(""); }}><item.icon size={16} />{item.label}</button>)}</div>
    {kind === "account" ? <Input label="Account ID" placeholder="c:ada, c:ada.example, or si:scout" value={recipient} onChange={event => setRecipient(event.target.value)} required /> : organizations.error ? <div className={styles.notice}>Couldn’t load your organizations. <Button type="button" variant="ghost" onClick={() => void organizations.refetch()}>Try again</Button></div> : organizations.isPending ? <p role="status">Loading available teams…</p> : targets.length ? <><Select label={kind === "team" ? "Team" : "Organization"} placeholder={`Choose ${kind === "team" ? "a team" : "an organization"}`} value={targetId} onValueChange={setTargetId} options={targets.map(target => ({ value: target.id, label: kind === "team" ? `${target.name} · ${target.org_id}` : target.name }))} /><p className={styles.muted}>{kind === "team" ? "Current and future members of this team receive access. Leaving the team ends access from this share." : "Current and future members of this organization receive access."}</p></> : <div className={styles.notice}>{kind === "team" ? "No teams are available to this account." : "This account has no organization membership."} Open Organizations or switch to your work account to manage team sharing.</div>}
    <div className={styles.fields}><Select label="Access" value={rights} onValueChange={setRights} disabled={!!minutes} options={[{ value: "read", label: "Can view and download" }, { value: "read,update", label: "Can view and edit" }, ...(folder ? [{ value: "read,write", label: "Can view and add files" }, { value: "read,write,update", label: "Can view, add, and edit" }] : [])]} /><Input label="Expires after (minutes)" type="number" min={1} max={43200} step={1} placeholder="No expiry" value={minutes} onChange={event => setMinutes(event.target.value)} /></div>
    {minutes && <p className={styles.muted}>Expiring shares allow viewing and downloading only, for up to 30 days.</p>}
    <Button type="submit" loading={busy} disabled={kind !== "account" && (!target || !!organizations.error)}>Share {kind === "team" ? "with team" : kind === "organization" ? "with organization" : "access"}</Button>
  </form>;
}
