/**
 * GET /auth/callback?code=…&state=… (or ?error=…&state=…): the end of a sign-in. The state must be one this browser
 * started (the sealed pending cookie); the code is exchanged server side with the app secret and its PKCE verifier, and
 * the tokens are sealed into the session cookie. The browser then goes back to where it started.
 *
 * Failures land on /sign-in?error=<code>, which shows fixed words per code (error=access_denied, a Carbon who cancelled,
 * gets a friendly page): the address's own error_description is never shown, since anyone could put words in a link.
 * The account that was current stays signed in on this browser for the account menu (lib/server/accounts.ts). A session
 * of the same account that this sign-in replaces is revoked at Silicon Accounts, so no old sign-in lingers.
 */
import type { NextRequest } from "next/server";
import { forget, rememberable } from "@/lib/server/accounts";
import { EnvError, serverEnv } from "@/lib/server/env";
import { misconfigured, redirectTo } from "@/lib/server/responses";
import { clientHeaders, readOtherAccounts, readPending, readSession, rememberAccount, writeOtherAccounts, writePending, writeSession, type OtherAccount, type PendingSignIn } from "@/lib/server/session";
import { exchangeCode, revokeSession } from "@/lib/server/tokens";

export const dynamic = "force-dynamic";

const ERROR_CODE = /^[a-z_]{1,48}$/;

function failed(publicUrl: string, code: string, pending: PendingSignIn[], state: string | null, returnTo?: string) {
  const target = new URL("/sign-in", publicUrl);
  target.searchParams.set("error", ERROR_CODE.test(code) ? code : "sign_in_failed");
  if (returnTo && returnTo !== "/") target.searchParams.set("return_to", returnTo);
  const response = redirectTo(target);
  writePending(response, pending.filter(entry => entry.s !== state));
  return response;
}

export async function GET(request: NextRequest) {
  let env;
  try {
    env = serverEnv();
  } catch (error) {
    if (error instanceof EnvError) return misconfigured(error);
    throw error;
  }
  const params = request.nextUrl.searchParams;
  const state = params.get("state");
  const pending = readPending(request);
  const entry = state ? pending.find(item => item.s === state) : undefined;
  if (!entry) return failed(env.publicUrl, "state_mismatch", pending, state);
  const error = params.get("error");
  if (error) return failed(env.publicUrl, error, pending, state, entry.r);
  const code = params.get("code");
  if (!code) return failed(env.publicUrl, "missing_code", pending, state, entry.r);

  const headers = clientHeaders(request);
  const result = await exchangeCode(code, entry.v, headers);
  if (!result.ok) {
    console.warn(`sign-in: the code exchange failed: ${result.status} ${result.error}: ${result.description}`);
    return failed(env.publicUrl, result.error === "accounts_unreachable" ? "accounts_unreachable" : "exchange_failed", pending, state, entry.r);
  }
  const replaced = readSession(request);
  let previous: OtherAccount | null = null;
  if (replaced && replaced.acct.uuid === result.session.acct.uuid) {
    if (replaced.rt !== result.session.rt) await revokeSession(replaced, headers);
  } else {
    previous = await rememberable(replaced, headers);
  }
  const { kept, dropped } = rememberAccount(readOtherAccounts(request), previous, { uuid: result.session.acct.uuid, sid: result.session.sid });
  await Promise.all(dropped.map(item => forget(item, headers)));

  const response = redirectTo(new URL(entry.r, env.publicUrl));
  writeSession(response, result.session);
  writeOtherAccounts(response, kept);
  writePending(response, pending.filter(item => item.s !== state));
  return response;
}
