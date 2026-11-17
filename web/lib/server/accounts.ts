/**
 * The other accounts signed in on this browser (lib/server/session.ts OtherAccount), on the server: a remembered
 * account's session is found by its durable id, the account that stops being current is kept with its organization, and
 * an account that falls off the list has its sign-in ended at Silicon Accounts.
 */
import { serverEnv } from "./env";
import { freshSession } from "./fresh";
import { locateSession } from "./refresh-state";
import type { OtherAccount, StoredSession } from "./session";
import { revokeSession } from "./tokens";

/** A remembered account's session as the token code reads it: the durable store holds its tokens under `sid`. */
export function storedFor(account: OtherAccount): StoredSession {
  return { v: 1, sid: account.sid, at: "", rt: "", ae: 0, re: account.re, scope: "", acct: account.acct };
}

/** A work account's organization (name and logo) for the account menu; null for a personal account or when unknown. */
export async function organizationFor(session: StoredSession): Promise<OtherAccount["org"]> {
  try {
    const response = await fetch(`${serverEnv().appApiUrl}/v1/organizations`, {
      headers: { Authorization: `Bearer ${session.at}`, Accept: "application/json" },
      cache: "no-store",
      signal: AbortSignal.timeout(4_000),
    });
    if (!response.ok) return null;
    const body = (await response.json()) as { organizations?: { name?: unknown; logo_url?: unknown }[] };
    const org = body.organizations?.[0];
    return org && typeof org.name === "string" ? { name: org.name, logo_url: typeof org.logo_url === "string" && org.logo_url ? org.logo_url : null } : null;
  } catch {
    return null;
  }
}

/** The current session as an account to remember, or null when its sign-in has ended (or cannot be checked now). */
export async function rememberable(session: StoredSession | null, headers: Record<string, string>): Promise<OtherAccount | null> {
  if (!session) return null;
  try {
    const state = await freshSession(session, "api", headers);
    if (state.kind !== "fresh" || !state.session.sid) return null;
    return { sid: state.session.sid, re: state.session.re, acct: state.session.acct, org: await organizationFor(state.session) };
  } catch {
    return null;
  }
}

/** Ends a remembered account's sign-in, at Silicon Accounts and in the store. Best effort: a gone session is fine. */
export async function forget(account: OtherAccount, headers: Record<string, string>): Promise<void> {
  try {
    const { entry } = await locateSession(storedFor(account));
    await revokeSession(entry.value.session, headers);
  } catch {
    // Already ended, or the store will let it expire.
  }
}
