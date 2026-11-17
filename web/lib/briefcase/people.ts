import type { ChipAccount } from "@/components/foundation/account/account-chip";

export type Person = ChipAccount & {
  uuid: string;
  invited?: number;
  received?: number;
  last_invited_at?: string | null;
  contact?: { name: string; trust: number; photo_id: string | null };
};
export type People = { items: Person[]; contacts_status: "connected" | "disconnected" | "unavailable" };
export type Contact = {
  uuid: string; account: Person; name: string; trust: number; photo_id: string | null;
};
export type Contacts = { connected: boolean; items: Contact[] };
export function contactPerson(contact: Contact): Person {
  return { ...contact.account, uuid: contact.uuid, id: contact.account.id || contact.uuid, display_name: contact.name,
    pfp_url: contact.photo_id ? `/api/v1/contacts/photos/${encodeURIComponent(contact.photo_id)}` : contact.account.pfp_url,
    fallback_pfp_url: contact.account.pfp_url,
    contact: { name: contact.name, trust: contact.trust, photo_id: contact.photo_id } };
}
export const inviteScore = (person: Person) => 2 * (person.invited ?? 0) + (person.received ?? 0);
const normalized = (s: string) => s.normalize("NFKD").replace(/\p{M}/gu, "").toLocaleLowerCase().trim();

// Damerau–Levenshtein catches swapped letters as well as insertions/deletions.
function distance(a: string, b: string): number {
  const rows = Array.from({ length: a.length + 1 }, () => Array<number>(b.length + 1).fill(0));
  for (let i = 0; i <= a.length; i++) rows[i]![0] = i;
  for (let j = 0; j <= b.length; j++) rows[0]![j] = j;
  for (let i = 1; i <= a.length; i++) for (let j = 1; j <= b.length; j++) {
    rows[i]![j] = Math.min(rows[i - 1]![j]! + 1, rows[i]![j - 1]! + 1, rows[i - 1]![j - 1]! + Number(a[i - 1] !== b[j - 1]));
    if (i > 1 && j > 1 && a[i - 1] === b[j - 2] && a[i - 2] === b[j - 1]) rows[i]![j] = Math.min(rows[i]![j]!, rows[i - 2]![j - 2]! + 1);
  }
  return rows[a.length]![b.length]!;
}
export function matchesPerson(person: Person, query: string): boolean {
  const needle = normalized(query).slice(0, 200);
  if (!needle) return true;
  if ((needle.startsWith("si:") && person.kind !== "silicon") || (needle.startsWith("c:") && person.kind !== "carbon")) return false;
  const haystack = normalized(`${person.id} ${person.display_name ?? ""} ${person.contact?.name ?? ""}`);
  const words = haystack.split(/[^\p{L}\p{N}_-]+/u).filter(Boolean);
  return needle.split(/\s+/).every(part => {
    if (haystack.includes(part)) return true;
    const word = part.replace(/^(c|si):/, "");
    if (word.length < 3) return false;
    const edits = word.length >= 7 ? 2 : 1;
    return words.some(candidate => {
      if (Math.abs(candidate.length - word.length) <= edits && distance(word, candidate) <= edits) return true;
      if (candidate.length > word.length + 4) return false;
      let index = 0;
      for (const letter of candidate) if (letter === word[index]) index++;
      return index === word.length;
    });
  });
}

/** Filtering never changes the requested ranking: weighted history, recency, then trust. */
export function suggestPeople(people: Person[], query = "", self?: string, limit = 12): Person[] {
  const unique = new Map(people.filter(p => p.uuid !== self && p.id && matchesPerson(p, query)).map(p => [p.uuid, p]));
  return [...unique.values()].sort((a, b) =>
    inviteScore(b) - inviteScore(a)
    || (Date.parse(b.last_invited_at ?? "") || 0) - (Date.parse(a.last_invited_at ?? "") || 0)
    || (b.contact?.trust ?? 0) - (a.contact?.trust ?? 0)
    || (a.display_name || a.id).localeCompare(b.display_name || b.id)
    || a.uuid.localeCompare(b.uuid)).slice(0, limit);
}
export function suggestionNote(person: Person): string {
  if ((person.invited ?? 0) + (person.received ?? 0) > 1) return "Frequently invited";
  if (inviteScore(person)) return "Recent invite";
  return person.contact ? `Contact · Trust ${person.contact.trust}` : "Silicon Accounts";
}
