/**
 * POST /auth/accounts/switch {"account_uuid": "…"}: makes another account signed in on this browser the current one. Its
 * durable session is checked (and refreshed when due) first; the account that was current joins the list, so switching
 * back is one more choice. Same-origin only (CSRF guard). 204 on success; 409 `sign_in_ended` when that account's
 * sign-in has ended (it leaves the list; sign in to it again).
 */
import { NextResponse, type NextRequest } from "next/server";
import { forget, rememberable, storedFor } from "@/lib/server/accounts";
import { EnvError, serverEnv } from "@/lib/server/env";
import { freshSession } from "@/lib/server/fresh";
import { json, misconfigured } from "@/lib/server/responses";
import { clientHeaders, errorBody, readOtherAccounts, readSession, rememberAccount, sameOriginProblem, writeOtherAccounts, writeSession } from "@/lib/server/session";

export const dynamic = "force-dynamic";

export async function POST(request: NextRequest) {
  try {
    serverEnv();
  } catch (error) {
    if (error instanceof EnvError) return misconfigured(error);
    throw error;
  }
  const problem = sameOriginProblem(request);
  if (problem) return json(403, errorBody("cross_site_request", problem, "Switch accounts from this site's own account menu."));
  let uuid: unknown;
  try {
    uuid = ((await request.json()) as { account_uuid?: unknown } | null)?.account_uuid;
  } catch {
    uuid = undefined;
  }
  if (typeof uuid !== "string" || !uuid) return json(422, errorBody("invalid_request", "Name the account to switch to in account_uuid."));

  const headers = clientHeaders(request);
  const others = readOtherAccounts(request);
  const chosen = others.find(item => item.acct.uuid === uuid);
  if (!chosen) return json(404, errorBody("unknown_account", "That account is not signed in on this browser.", "Add it with Add Another Account."));
  const rest = others.filter(item => item !== chosen);
  const state = await freshSession(storedFor(chosen), "page", headers);
  if (state.kind === "unavailable") return json(state.status, errorBody(state.code, state.message));
  if (state.kind === "signed_out") {
    return json(409, errorBody("sign_in_ended", `The sign-in for ${chosen.acct.id} has ended.`, "Sign in to it again with Add Another Account."), response => writeOtherAccounts(response, rest));
  }

  const previous = await rememberable(readSession(request), headers);
  const { kept, dropped } = rememberAccount(rest, previous, { uuid: state.session.acct.uuid, sid: state.session.sid });
  await Promise.all(dropped.map(item => forget(item, headers)));
  const response = new NextResponse(null, { status: 204, headers: { "Cache-Control": "no-store" } });
  writeSession(response, state.session);
  writeOtherAccounts(response, kept);
  return response;
}
