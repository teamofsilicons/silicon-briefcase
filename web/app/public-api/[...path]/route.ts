import { NextRequest, NextResponse } from "next/server";
import { serverEnv } from "@/lib/server/env";
import { relayResponseHeaders, upstreamPath } from "@/lib/server/forward";
export const dynamic = "force-dynamic";
export async function GET(request: NextRequest) {
 const rest = request.nextUrl.pathname.slice("/public-api/".length); const path = upstreamPath(rest);
 if (!path) return NextResponse.json({error:{code:"invalid_path",message:"This file address is invalid."}}, {status:400});
 const headers = new Headers(); for (const name of ["range","if-none-match","if-modified-since"]) { const value=request.headers.get(name); if(value) headers.set(name,value); }
 try {
  const response = await fetch(`${serverEnv().appApiUrl}/v1/public/${path}${request.nextUrl.search}`, { headers,redirect:"manual",cache:"no-store",signal:AbortSignal.any([request.signal,AbortSignal.timeout(900_000)]) });
  if(response.status === 308) { const body=await response.json() as {permanent_url:string}; const destination=new URL(body.permanent_url); return NextResponse.redirect(new URL(`/public-api${destination.pathname}${request.nextUrl.search}`,request.url),308); }
  const outgoing=relayResponseHeaders(response.headers);
  // Keep uploaded HTML, SVG and document bytes isolated even when opened directly.
  outgoing.set("content-security-policy","default-src 'none'; sandbox; style-src 'unsafe-inline'");
  return new NextResponse(response.status === 304 ? null : response.body,{status:response.status,headers:outgoing});
 } catch { return NextResponse.json({error:{code:"service_unreachable",message:"This file could not be reached. Try again shortly."}},{status:502}); }
}
