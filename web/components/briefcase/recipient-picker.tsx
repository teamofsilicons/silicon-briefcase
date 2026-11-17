"use client";
import Link from "next/link";
import { useEffect, useId, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Input } from "@/components/silicon-ui/input/input";
import { Button } from "@/components/silicon-ui/button/button";
import { AccountChip } from "@/components/foundation/account/account-chip";
import { api } from "@/lib/client/api";
import { useSessionView } from "@/lib/client/session";
import { parseAccountId } from "@/lib/ids";
import { message } from "@/lib/briefcase/types";
import { suggestPeople, suggestionNote, type Person } from "@/lib/briefcase/people";
import { usePeople } from "./people-query";
import styles from "./people.module.css";

export function RecipientPicker({ value, onChange, disabled, onSaveContact }: { value: Person | null; onChange: (value: Person | null) => void; disabled?: boolean; onSaveContact?: (person: Person) => void }) {
  const people = usePeople();
  const { account } = useSessionView();
  const [draft, setDraft] = useState(value?.id ?? "");
  const [settled, setSettled] = useState(draft);
  const [focused, setFocused] = useState(false);
  const [active, setActive] = useState(-1);
  const id = useId();
  useEffect(() => { const timer = setTimeout(() => setSettled(draft.trim()), 300); return () => clearTimeout(timer); }, [draft]);
  const parsed = parseAccountId(settled);
  const resolve = useQuery({ queryKey: ["recipient", account?.uuid, settled], queryFn: ({ signal }) => api.get<Person>("/v1/recipients/resolve", { query: { id: settled }, signal }), enabled: !!account && parsed.ok && settled === draft.trim() && !value, retry: false, staleTime: 30_000 });
  const resolved = settled === draft.trim() ? resolve.data : undefined;
  useEffect(() => {
    if (!value && resolved && resolved.uuid !== account?.uuid) onChange(resolved);
  }, [resolved, value, account?.uuid, onChange]);
  useEffect(() => { if (active >= 0) document.getElementById(`${id}-${active}`)?.scrollIntoView({ block: "nearest" }); }, [active, id]);
  const suggestions = suggestPeople(people.data?.items ?? [], draft, account?.uuid);
  const open = focused && !value && suggestions.length > 0;
  function choose(person: Person) { onChange(person); setDraft(person.id); setFocused(false); setActive(-1); }
  const error = resolved?.uuid === account?.uuid ? "This is your account. You already have access." : !value && parsed.ok && settled === draft.trim() && resolve.error ? message(resolve.error) : undefined;
  return <div className={styles.field}>
    <Input label="Account ID or name" placeholder="Search people, c:ada, or si:scout" value={draft} disabled={disabled}
      role="combobox" aria-autocomplete="list" aria-expanded={open} aria-controls={open ? id : undefined} aria-activedescendant={open && active >= 0 && suggestions[active] ? `${id}-${active}` : undefined}
      maxLength={200} autoComplete="off" autoCapitalize="none" spellCheck={false} error={error}
      onFocus={() => setFocused(true)} onBlur={() => setFocused(false)}
      onChange={event => { setDraft(event.target.value); onChange(null); setFocused(true); setActive(-1); }}
      onKeyDown={event => {
        if (event.key === "ArrowDown" && open) { event.preventDefault(); setActive(n => (n + 1) % suggestions.length); }
        else if (event.key === "ArrowUp" && open) { event.preventDefault(); setActive(n => (n <= 0 ? suggestions.length : n) - 1); }
        else if (event.key === "Enter" && open && active >= 0) { event.preventDefault(); choose(suggestions[active]!); }
        else if (event.key === "Enter" && !value) event.preventDefault();
        else if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setFocused(false); }
      }} />
    {open && <ul id={id} role="listbox" aria-label="Suggested people" className={styles.suggestions} data-sq="surface">{suggestions.map((person, index) => <li key={person.uuid} id={`${id}-${index}`} role="option" aria-selected={active === index} className={styles.suggestion} onMouseDown={event => event.preventDefault()} onClick={() => choose(person)}><AccountChip account={person} /><span className={styles.note}>{suggestionNote(person)}</span></li>)}</ul>}
    {!value && resolve.isFetching && settled === draft.trim() && <p role="status" className={styles.note}>Checking this account…</p>}
    {!value && focused && draft && !suggestions.length && !resolve.isFetching && !error && <p className={styles.note}>No matching suggestions. Enter a full Carbon or Silicon ID to find someone new.</p>}
    {value && <div className={styles.selected} data-sq="surface" aria-label="Selected recipient"><AccountChip account={value} note={value.contact ? `Trust ${value.contact.trust}` : undefined} />{onSaveContact && people.data?.contacts_status === "connected" && <Button type="button" size="sm" variant="ghost" onClick={() => onSaveContact(value)}>{value.contact ? "Edit contact" : "Add contact"}</Button>}</div>}
    {people.isPending && <p role="status" className={styles.note}>Loading suggestions…</p>}
    {people.error && <div className={styles.connection}><p>Invite history couldn’t load. You can still enter an exact account ID.</p><Button type="button" variant="ghost" size="sm" onClick={() => void people.refetch()}>Retry suggestions</Button></div>}
    {people.data?.contacts_status === "disconnected" && <p className={styles.connection}><Link href="/contacts">Connect Silicon Contacts</Link> to include your saved names, photos and trusted contacts.</p>}
    {people.data?.contacts_status === "unavailable" && <div className={styles.connection}><p>Contacts are unavailable. Suggestions show invitation history.</p><Button type="button" variant="ghost" size="sm" onClick={() => void people.refetch()}>Retry contacts</Button></div>}
  </div>;
}
