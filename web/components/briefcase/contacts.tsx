"use client";
import Link from "next/link";
import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Plus } from "lucide-react";
import { Button } from "@/components/silicon-ui/button/button";
import { Input } from "@/components/silicon-ui/input/input";
import { Select } from "@/components/silicon-ui/select/select";
import { Dialog, DialogContent } from "@/components/silicon-ui/dialog/dialog";
import { EmptyState } from "@/components/silicon-ui/empty-state/empty-state";
import { Page, PageHeader } from "@/components/foundation/layout/layout";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { AccountChip } from "@/components/foundation/account/account-chip";
import { api } from "@/lib/client/api";
import { notify } from "@/lib/notify";
import { message } from "@/lib/briefcase/types";
import { contactPerson, matchesPerson, type Contact, type Person } from "@/lib/briefcase/people";
import { useContacts } from "./people-query";
import { RecipientPicker } from "./recipient-picker";
import styles from "./people.module.css";

export function ContactEditor({ initial, onClose, onSaved }: { initial?: Person; onClose: () => void; onSaved?: (person: Person) => void }) {
  const cache = useQueryClient();
  const [person, setPerson] = useState<Person | null>(initial ?? null);
  const [name, setName] = useState(initial?.display_name ?? "");
  const [trust, setTrust] = useState(String(initial?.contact?.trust ?? 1));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function save(event: React.FormEvent) {
    event.preventDefault(); event.stopPropagation();
    if (!person) return;
    setBusy(true); setError("");
    try {
      const level = Number(trust);
      if (!Number.isInteger(level) || level < 1 || level > 255) throw new Error("Choose a whole trust level from 1 to 255.");
      const contact = await api.post<Contact>("/v1/contacts", { id: person.uuid, name: name.trim(), trust: level });
      await Promise.all([cache.invalidateQueries({ queryKey: ["contacts"] }), cache.invalidateQueries({ queryKey: ["people"] }), cache.invalidateQueries({ queryKey: ["recipient"] }), cache.invalidateQueries({ queryKey: ["files", "shared"] })]);
      onSaved?.(contactPerson(contact)); notify.success("Contact saved in Silicon Contacts"); onClose();
    } catch (failure) { setError(message(failure)); } finally { setBusy(false); }
  }
  return <Dialog open onOpenChange={open => { if (!open) onClose(); }}><DialogContent title={initial?.contact ? "Edit contact" : "Add contact"} description="Save a name and trust level in your Silicon Contacts address book."><form className={styles.form} onSubmit={event => void save(event)}>
    {initial ? <AccountChip account={person ?? initial} /> : <RecipientPicker value={person} disabled={busy} onChange={value => { setPerson(value); if (value) { setName(value.display_name || value.id); setTrust(String(value.contact?.trust ?? 1)); } }} />}
    <Input label="Saved name" value={name} onChange={event => setName(event.target.value)} required maxLength={200} disabled={busy} />
    <Input label="Trust level" type="number" min={1} max={255} step={1} value={trust} onChange={event => setTrust(event.target.value)} required description="1 · In contacts. 2 · Trusted. You can use levels up to 255; higher levels appear first." disabled={busy} />
    <div className={styles.actions}><Button type="button" variant="secondary" size="sm" onClick={() => setTrust("1")}>In contacts · 1</Button><Button type="button" variant="secondary" size="sm" onClick={() => setTrust("2")}>Trusted · 2</Button></div>
    {error && <p role="alert">{error}</p>}
    <Button type="submit" disabled={!person || !name.trim()} loading={busy}>Save contact</Button>
  </form></DialogContent></Dialog>;
}

export function ContactsPage() {
  const contacts = useContacts();
  const cache = useQueryClient();
  const [search, setSearch] = useState("");
  const [kind, setKind] = useState("all");
  const [editing, setEditing] = useState<Person | "new" | null>(null);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  async function connect(enabled: boolean) {
    setBusy(true); setFailure("");
    try {
      await api.put("/v1/contacts/connection", { enabled });
      // Clear private contact projections, including previously resolved recipient pictures.
      cache.removeQueries({ queryKey: ["recipient"] });
      cache.removeQueries({ queryKey: ["people"] });
      await Promise.all([cache.invalidateQueries({ queryKey: ["contacts"] }), cache.invalidateQueries({ queryKey: ["files", "shared"] })]);
    } catch (error) { setFailure(message(error)); } finally { setBusy(false); }
  }
  const items = (contacts.data?.items ?? []).map(contactPerson).filter(person => (kind === "all" || person.kind === kind) && matchesPerson(person, search)).sort((a, b) => (b.contact?.trust ?? 0) - (a.contact?.trust ?? 0) || (a.display_name || a.id).localeCompare(b.display_name || b.id));
  return <Page><PageHeader title="Contacts" description="Your Silicon Contacts address book, here in Briefcase. Saved names and photos follow you into sharing." actions={contacts.data?.connected ? <Button onClick={() => setEditing("new")}><Plus size={17} />Add contact</Button> : undefined} />
    {failure && <p role="alert">{failure}</p>}
    {contacts.error && <ErrorAlert error={contacts.error} action={<Button variant="secondary" onClick={() => void contacts.refetch()}>Try again</Button>} />}
    {contacts.isPending ? <p role="status">Loading contacts…</p> : contacts.data?.connected === false ? <div className={styles.connection}><h2>Bring your contacts into Briefcase</h2><p>Allow Briefcase to read and save contacts, including names, photos and trust levels. Changes here are saved in Silicon Contacts for this account.</p><div className={styles.actions}><Button loading={busy} onClick={() => void connect(true)}>Connect Silicon Contacts</Button><a href="https://contacts.teamofsilicons.com/" target="_blank" rel="noreferrer">Open Silicon Contacts</a></div></div> : contacts.data?.connected ? <>
      <div className={styles.filters}><Input label="Search contacts" placeholder="Name or account ID…" value={search} onChange={event => setSearch(event.target.value)} /><Select label="Account type" value={kind} onValueChange={setKind} options={[{ value: "all", label: "Carbons and Silicons" }, { value: "carbon", label: "Carbons" }, { value: "silicon", label: "Silicons" }]} /></div>
      <p className={styles.note}>Highest trust first. <Link href="/shared?contacts=saved">See files shared by contacts</Link></p>
      {items.length ? <ul className={styles.list}>{items.map(person => <li key={person.uuid} className={styles.row}><AccountChip account={person} note={`Trust ${person.contact?.trust}`} /><div className={styles.actions}><Link href={`/shared?from=${encodeURIComponent(person.uuid)}`}>Shared files</Link><Button variant="ghost" size="sm" onClick={() => setEditing(person)}>Edit contact</Button></div></li>)}</ul> : <EmptyState title={search ? "No matching contacts" : "Add someone you work with"} description={search ? "Try a different name or account ID." : "Save a Carbon or Silicon and choose how much you trust them."} />}
      <div className={styles.actions}><a href="https://contacts.teamofsilicons.com/" target="_blank" rel="noreferrer">Manage photos and other details in Silicon Contacts</a><Button variant="ghost" loading={busy} onClick={() => void connect(false)}>Disconnect Contacts</Button></div>
    </> : null}
    {editing && <ContactEditor initial={editing === "new" ? undefined : editing} onClose={() => setEditing(null)} />}
  </Page>;
}
