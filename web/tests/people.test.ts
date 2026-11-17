import { test } from "node:test";
import assert from "node:assert/strict";
import { contactPerson, matchesPerson, suggestPeople, type Person } from "../lib/briefcase/people";
const person = (uuid: string, extra: Partial<Person> = {}): Person => ({ uuid, id: `c:${uuid}`, kind: "carbon", ...extra });
test("weighted outgoing/incoming history wins, then recent invites, then contact trust", () => {
  const contact = (trust: number) => ({ name: "Contact", trust, photo_id: null });
  const rows = [person("trusted", { contact: contact(255) }), person("once", { received: 1 }), person("incoming", { received: 4, last_invited_at: "2026-11-16" }), person("outgoing", { invited: 3, last_invited_at: "2026-11-01" }), person("recent", { invited: 2, last_invited_at: "2026-11-17" }), person("saved", { contact: contact(1) })];
  assert.deepEqual(suggestPeople(rows).map(p => p.uuid), ["outgoing", "recent", "incoming", "once", "trusted", "saved"]);
  assert.deepEqual(suggestPeople(rows, "", "outgoing", 2).map(p => p.uuid), ["recent", "incoming"]);
});
test("fuzzy filtering handles transposition, partial IDs, accents and multiword names without reordering by match quality", () => {
  const grace = person("grace", { display_name: "Gráce Hopper", invited: 10 });
  assert.equal(matchesPerson(grace, "garce"), true);
  assert.equal(matchesPerson(grace, "grce hop"), true);
  assert.equal(matchesPerson(grace, "c:gra"), true);
  assert.equal(matchesPerson(grace, "grace smith"), false);
  assert.equal(matchesPerson(grace, "zz"), false);
  assert.deepEqual(suggestPeople([person("garce"), grace], "garce").map(p => p.uuid), ["grace", "garce"]);
});
test("saved details override the profile without changing immutable identity", () => {
  const saved = contactPerson({ uuid: "stable", account: person("stable", { display_name: "Public", pfp_url: "https://example.test/public.png" }), name: "My friend", trust: 7, photo_id: "photo" });
  assert.equal(saved.uuid, "stable"); assert.equal(saved.display_name, "My friend");
  assert.equal(saved.pfp_url, "/api/v1/contacts/photos/photo"); assert.equal(saved.contact?.trust, 7);
});
