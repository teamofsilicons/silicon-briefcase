/**
 * GET /auth/accounts: the other accounts signed in to this app in this browser, for the account menu (as Silicon Accounts'
 * profile menu lists its browser profiles). Each is the account as the menu shows it (name, id, photo) and, for a work
 * account, its organization; never a token. POST /auth/accounts/switch makes one current.
 */
import type { NextRequest } from "next/server";
import { EnvError, serverEnv } from "@/lib/server/env";
import { json, misconfigured } from "@/lib/server/responses";
import { errorBody, readOtherAccounts, readSession } from "@/lib/server/session";

export const dynamic = "force-dynamic";

export function GET(request: NextRequest) {
  try {
    serverEnv();
  } catch (error) {
    if (error instanceof EnvError) return misconfigured(error);
    throw error;
  }
  const current = readSession(request);
  if (!current) return json(401, errorBody("signed_out", "This browser is not signed in."));
  const accounts = readOtherAccounts(request)
    .filter(item => item.acct.uuid !== current.acct.uuid)
    .map(item => ({ uuid: item.acct.uuid, kind: item.acct.kind, id: item.acct.id, display_name: item.acct.display_name, pfp_url: item.acct.pfp_url, org: item.org }));
  return json(200, { accounts });
}
