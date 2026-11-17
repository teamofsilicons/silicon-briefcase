"use client";

/**
 * Contacts: the account's Silicon Contacts address book in Briefcase, on Accounts' page rules (the page bar, a line of
 * context, the contacts as one boxed list with their photo, name and trust), and the dialog that adds or edits one.
 */
import Link from "next/link";
import { useState, type FormEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ContactRound, Plus } from "lucide-react";
import { Alert } from "@teamofsilicons/silicon-ui/alert";
import { Avatar } from "@teamofsilicons/silicon-ui/avatar";
import { Button } from "@teamofsilicons/silicon-ui/button";
import { ButtonLink } from "@teamofsilicons/silicon-ui/button-link";
import { Input } from "@teamofsilicons/silicon-ui/input";
import { Select } from "@teamofsilicons/silicon-ui/select";
import { Dialog, DialogContent } from "@teamofsilicons/silicon-ui/dialog";
import { EmptyState } from "@teamofsilicons/silicon-ui/empty-state";
import { Page } from "@/components/foundation/layout/layout";
import { PageBar } from "@/components/foundation/page/page-bar";
import { Row, Section, Tile } from "@/components/foundation/page/rows";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import { AccountChip } from "@/components/foundation/account/account-chip";
import { SkeletonBlock } from "@/components/foundation/feedback/skeleton-block";
import { api } from "@/lib/client/api";
import { kindNoun } from "@/lib/format";
import { notify } from "@/lib/notify";
import { message } from "@/lib/briefcase/types";
import { contactPerson, matchesPerson, type Contact, type Person } from "@/lib/briefcase/people";
import { useContacts } from "./people-query";
import { RecipientPicker } from "./recipient-picker";
import styles from "./people.module.css";

const CONTACTS_SITE = "https://contacts.teamofsilicons.com/";

export function ContactEditor({ initial, onClose, onSaved }: { initial?: Person; onClose: () => void; onSaved?: (person: Person) => void }) {
  const cache = useQueryClient();
  const [person, setPerson] = useState<Person | null>(initial ?? null);
  const [name, setName] = useState(initial?.display_name ?? "");
  const [trust, setTrust] = useState(String(initial?.contact?.trust ?? 1));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function save(event: FormEvent) {
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
  return <Dialog open onOpenChange={open => { if (!open) onClose(); }}>
    <DialogContent title={initial?.contact ? "Edit Contact" : "Add Contact"} description="Save a name and trust level in your Silicon Contacts address book.">
      <form className={styles.form} onSubmit={event => void save(event)}>
        {initial ? <AccountChip account={person ?? initial} /> : <RecipientPicker value={person} disabled={busy} onChange={value => { setPerson(value); if (value) { setName(value.display_name || value.id); setTrust(String(value.contact?.trust ?? 1)); } }} />}
        <Input label="Saved name" value={name} onChange={event => setName(event.target.value)} required maxLength={200} disabled={busy} />
        <Input label="Trust level" type="number" min={1} max={255} step={1} value={trust} onChange={event => setTrust(event.target.value)} required description="1 · In contacts. 2 · Trusted. You can use levels up to 255; higher levels appear first." disabled={busy} />
        <div className={styles.presets}>
          <Button type="button" variant="secondary" size="xs" onClick={() => setTrust("1")}>In Contacts · 1</Button>
          <Button type="button" variant="secondary" size="xs" onClick={() => setTrust("2")}>Trusted · 2</Button>
        </div>
        {error ? <Alert tone="danger" title="The contact was not saved">{error}</Alert> : null}
        <div className={styles.dialogActions}>
          <Button type="button" variant="ghost" onClick={onClose}>Cancel</Button>
          <Button type="submit" disabled={!person || !name.trim()} loading={busy}>Save Contact</Button>
        </div>
      </form>
    </DialogContent>
  </Dialog>;
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
  return <>
    <PageBar title="Contacts" actions={contacts.data?.connected ? <Button size="sm" onClick={() => setEditing("new")}><Plus size={16} strokeWidth={2} aria-hidden="true" />Add Contact</Button> : undefined} />
    <Page>
      <p className={styles.lead}>Your Silicon Contacts address book, here in Briefcase. Saved names and photos follow you into sharing.</p>
      {failure ? <Alert tone="danger" title="That did not work" onDismiss={() => setFailure("")}>{failure}</Alert> : null}
      {contacts.error ? <ErrorAlert error={contacts.error} action={<Button variant="secondary" size="sm" onClick={() => void contacts.refetch()}>Try Again</Button>} /> : null}
      {contacts.isPending ? <div role="status" aria-label="Loading contacts"><SkeletonBlock width="100%" height="calc(3 * 3.25rem)" radius="var(--radius-container)" /></div>
        : contacts.data?.connected === false ? (
          <Section id="connect" title="Silicon Contacts">
            <Row wide lead={<Tile><ContactRound size={16} strokeWidth={2} /></Tile>} title="Bring your contacts into Briefcase" detail="Names, photos and trust levels. Changes here are saved in Silicon Contacts for this account.">
              <ButtonLink href={CONTACTS_SITE} target="_blank" rel="noreferrer" variant="ghost" size="sm">Open Silicon Contacts</ButtonLink>
              <Button size="sm" loading={busy} onClick={() => void connect(true)}>Connect Silicon Contacts</Button>
            </Row>
          </Section>
        ) : contacts.data?.connected ? <>
          <div className={styles.filters}>
            <Input label="Search contacts" placeholder="Name or account ID…" value={search} onChange={event => setSearch(event.target.value)} />
            <Select label="Account type" value={kind} onValueChange={setKind} options={[{ value: "all", label: "Carbons and Silicons" }, { value: "carbon", label: "Carbons" }, { value: "silicon", label: "Silicons" }]} />
          </div>
          {items.length ? (
            <Section id="contacts" title="Your contacts" count={items.length} description={<>Highest trust first. <Link href="/shared?contacts=saved">See files shared by contacts</Link></>}>
              {items.map(person => (
                <Row key={person.uuid} wide lead={<Avatar name={person.display_name || person.id} src={person.pfp_url ?? person.fallback_pfp_url ?? undefined} size="sm" />} title={person.display_name || person.id} detail={`${person.id} · ${kindNoun(person.kind)} · Trust ${person.contact?.trust}`}>
                  <ButtonLink href={`/shared?from=${encodeURIComponent(person.uuid)}`} variant="ghost" size="sm">Shared Files</ButtonLink>
                  <Button variant="secondary" size="sm" onClick={() => setEditing(person)}>Edit Contact</Button>
                </Row>
              ))}
            </Section>
          ) : (
            <div className={styles.empty}><EmptyState title={search ? "No matching contacts" : "Add someone you work with"} description={search ? "Try a different name or account ID." : "Save a Carbon or Silicon and choose how much you trust them."} /></div>
          )}
          <Section id="connection" title="Connection">
            <Row wide lead={<Tile><ContactRound size={16} strokeWidth={2} /></Tile>} title="Silicon Contacts" detail="Connected. Photos and other details are managed in Silicon Contacts.">
              <ButtonLink href={CONTACTS_SITE} target="_blank" rel="noreferrer" variant="ghost" size="sm">Manage</ButtonLink>
              <Button variant="secondary" size="sm" loading={busy} onClick={() => void connect(false)}>Disconnect Contacts</Button>
            </Row>
          </Section>
        </> : null}
      {editing ? <ContactEditor initial={editing === "new" ? undefined : editing} onClose={() => setEditing(null)} /> : null}
    </Page>
  </>;
}
