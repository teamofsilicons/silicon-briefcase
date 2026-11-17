/** The other accounts signed in on this browser: the list's rules, its cookie, and switching (POST /auth/accounts/switch). */
import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { NextRequest, NextResponse } from "next/server";
import { POST as switchAccount } from "../app/auth/accounts/switch/route";
import { GET as listAccounts } from "../app/auth/accounts/route";
import { createSession, endSession } from "../lib/server/refresh-state";
import {
  MAX_OTHER_ACCOUNTS, readOtherAccounts, readSession, rememberAccount, sealSession, writeOtherAccounts,
  type OtherAccount, type StoredSession,
} from "../lib/server/session";
import { seal } from "../lib/server/seal";
import { SECRET, applyTestEnv, json, mockFetch } from "./env";

const BASE = "http://127.0.0.1:4260";
const person = (n: number, id = `c:p${n}`) => ({ uuid: `uuid-${n}`, kind: "carbon" as const, id, display_name: `Person ${n}`, pfp_url: null, custodian: null });
const other = (n: number, sid = `sid-${n}`, re = Date.now() + 86_400_000): OtherAccount => ({ sid, re, acct: person(n), org: null });
const stored = (n: number, id?: string): StoredSession => ({ v: 1, at: `local-at-${n}`, rt: `sar_${n}`, ae: Date.now() + 30 * 60_000, re: Date.now() + 86_400_000, scope: "profile", acct: person(n, id) });
const cookieOf = (response: Response, name: string) => /=([^;]*)/.exec((response.headers.getSetCookie?.() ?? []).find(line => line.startsWith(`${name}=`)) ?? "")?.[1];

let restore = () => {};
beforeEach(() => applyTestEnv());
afterEach(() => restore());

test("the list keeps one entry per account, newest first, never the current account, and ends what falls off", () => {
  const list = [other(2), other(3)];
  const { kept, dropped } = rememberAccount(list, other(1), { uuid: "uuid-4", sid: "sid-4" });
  assert.deepEqual(kept.map(item => item.acct.uuid), ["uuid-1", "uuid-2", "uuid-3"]);
  assert.deepEqual(dropped, []);

  const again = rememberAccount([other(1, "old-1"), other(2)], other(1, "new-1"), { uuid: "uuid-3" });
  assert.deepEqual(again.kept.map(item => item.sid), ["new-1", "sid-2"], "the newer sign-in of an account wins");
  assert.deepEqual(again.dropped.map(item => item.sid), ["old-1"], "and the older one is ended");

  const current = rememberAccount([other(2), other(3, "sid-3")], other(1), { uuid: "uuid-3", sid: "sid-9" });
  assert.deepEqual(current.kept.map(item => item.acct.uuid), ["uuid-1", "uuid-2"], "the new current account leaves the list");
  assert.deepEqual(current.dropped.map(item => item.sid), ["sid-3"], "its older sign-in is ended");

  const same = rememberAccount([other(2, "sid-x")], null, { uuid: "uuid-2", sid: "sid-x" });
  assert.deepEqual(same, { kept: [], dropped: [] }, "the new current session itself is never ended");

  const full = rememberAccount(Array.from({ length: MAX_OTHER_ACCOUNTS }, (_, i) => other(i + 2)), other(1), { uuid: "uuid-99" });
  assert.equal(full.kept.length, MAX_OTHER_ACCOUNTS);
  assert.deepEqual(full.dropped.map(item => item.acct.uuid), [`uuid-${MAX_OTHER_ACCOUNTS + 1}`], "past the limit, the oldest is ended");
});

test("the list's cookie is sealed, httpOnly, drops ended sign-ins, and is cleared when empty", () => {
  const response = NextResponse.next();
  writeOtherAccounts(response, [other(1), other(2, "sid-2", Date.now() - 1)]);
  const header = response.headers.get("set-cookie") ?? "";
  assert.match(header, /^briefcase_accounts=v1\./);
  assert.match(header, /HttpOnly/i);
  assert.doesNotMatch(header, /Person 1|uuid-1/, "nothing in it is readable");
  const value = /briefcase_accounts=([^;]+)/.exec(header)![1]!;
  const read = readOtherAccounts(new NextRequest(`${BASE}/files`, { headers: { cookie: `briefcase_accounts=${value}` } }));
  assert.deepEqual(read.map(item => item.sid), ["sid-1"]);
  assert.deepEqual(readOtherAccounts(new NextRequest(`${BASE}/files`, { headers: { cookie: `briefcase_accounts=${seal([other(3)], "session", SECRET)}` } })), [], "a value sealed for another cookie is refused");
  const cleared = NextResponse.next();
  writeOtherAccounts(cleared, []);
  assert.match(cleared.headers.get("set-cookie") ?? "", /briefcase_accounts=;.*Max-Age=0/i);
});

async function signedIn() {
  const ada = await createSession(stored(1, "c:ada.example"));
  const grace = await createSession(stored(2, "c:grace"));
  const list = NextResponse.next();
  writeOtherAccounts(list, [{ sid: grace.sid!, re: grace.re, acct: grace.acct, org: null }]);
  const accounts = /briefcase_accounts=([^;]+)/.exec(list.headers.get("set-cookie") ?? "")![1]!;
  return { ada, grace, cookie: `briefcase_session=${sealSession(ada)}; briefcase_accounts=${accounts}` };
}
const post = (cookie: string, body: unknown, origin = BASE) =>
  new NextRequest(`${BASE}/auth/accounts/switch`, { method: "POST", headers: { cookie, origin, "content-type": "application/json", "sec-fetch-site": "same-origin" }, body: JSON.stringify(body) });

test("the menu lists the other accounts without a token", async () => {
  const { cookie } = await signedIn();
  const response = listAccounts(new NextRequest(`${BASE}/auth/accounts`, { headers: { cookie } }));
  assert.equal(response.status, 200);
  const body = await response.json();
  assert.deepEqual(body.accounts.map((item: { id: string }) => item.id), ["c:grace"]);
  assert.doesNotMatch(JSON.stringify(body), /sar_|local-at|sid/);
  assert.equal(listAccounts(new NextRequest(`${BASE}/auth/accounts`)).status, 401);
});

test("switching makes the other account current and keeps the last one, with its organization", async () => {
  const { ada, grace, cookie } = await signedIn();
  const calls = mockFetch(url => {
    assert.equal(url, "http://127.0.0.1:4261/v1/organizations", "only the service is asked, for the organization");
    return json(200, { organizations: [{ name: "Example Studio", logo_url: null }] });
  });
  restore = calls.restore;
  const response = await switchAccount(post(cookie, { account_uuid: grace.acct.uuid }));
  assert.equal(response.status, 204);
  const session = cookieOf(response, "briefcase_session")!;
  assert.equal(readSession(new NextRequest(`${BASE}/files`, { headers: { cookie: `briefcase_session=${session}` } }))?.acct.id, "c:grace");
  const list = readOtherAccounts(new NextRequest(`${BASE}/files`, { headers: { cookie: `briefcase_accounts=${cookieOf(response, "briefcase_accounts")}` } }));
  assert.deepEqual(list.map(item => [item.sid, item.acct.id, item.org?.name]), [[ada.sid, "c:ada.example", "Example Studio"]]);
});

test("switching refuses other sites, unknown accounts, and ended sign-ins (which leave the list)", async () => {
  const { grace, cookie } = await signedIn();
  restore = mockFetch(() => json(200, { organizations: [] })).restore;
  assert.equal((await switchAccount(post(cookie, { account_uuid: grace.acct.uuid }, "https://evil.example"))).status, 403);
  assert.equal((await switchAccount(post(cookie, { account_uuid: "uuid-nobody" }))).status, 404);
  assert.equal((await switchAccount(post(cookie, {}))).status, 422);
  await endSession(grace);
  const ended = await switchAccount(post(cookie, { account_uuid: grace.acct.uuid }));
  assert.equal(ended.status, 409);
  assert.equal((await ended.json()).error.code, "sign_in_ended");
  assert.match((ended.headers.getSetCookie?.() ?? []).join("\n"), /briefcase_accounts=;.*Max-Age=0/i, "the ended account left the list");
  assert.equal(cookieOf(ended, "briefcase_session"), undefined, "the current session is untouched");
});
