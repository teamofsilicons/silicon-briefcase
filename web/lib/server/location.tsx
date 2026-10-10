import "server-only";
import { cookies } from "next/headers";
import { notFound, redirect, permanentRedirect } from "next/navigation";
import { Providers } from "@/components/foundation/providers";
import { PublicViewer, type PublicEntry } from "@/components/briefcase/public-viewer";
import { isLocationOwner } from "@/lib/ids";
import type { Entry } from "@/lib/briefcase/types";
import { serverEnv } from "./env";
import { sessionCookieName,sessionFromCookie } from "./session";
export async function locationPage(owner:string,parts:string[],legacy=false) {
 // Route parameters in this Next build retain URL encoding. Decode once at
 // the boundary, then encode each segment for the upstream service.
 try { owner=decodeURIComponent(owner); parts=parts.map(part=>decodeURIComponent(part)); } catch { notFound(); }
 if([owner,...parts].some(part=>part.includes("/")||part.includes("\\")||part==="."||part==="..")) notFound();
 if(!legacy && !isLocationOwner(owner)) notFound();
 const path=parts.map(encodeURIComponent).join("/"); const env=serverEnv();
 const session=sessionFromCookie((await cookies()).get(sessionCookieName())?.value);
 const target=legacy?`${new URL(env.appApiUrl).origin}/org/${encodeURIComponent(owner)}/${path}`:`${env.appApiUrl}/v1/paths/${encodeURIComponent(owner)}${path?`/${path}`:""}`;
 const response=await fetch(target,{headers:session?{Authorization:`Bearer ${session.at}`}:{},cache:"no-store",redirect:"manual"});
 if(response.status===308){const moved=await response.json() as {permanent_url:string};permanentRedirect(new URL(moved.permanent_url).pathname);}
 if(!response.ok){
  // Private links deliberately return404 to anonymous callers. Give every
  // signed-out reader the same sign-in route, then enforce access after login.
  if(!session && (response.status===401 || response.status===404)) {
   const returnPath=`${legacy?"/org":""}/${encodeURIComponent(owner)}${path?`/${path}`:""}`;
   redirect(`/auth/sign-in?return_to=${encodeURIComponent(returnPath)}`);
  }
  notFound();
 }
 if(session){const entry=await response.json() as Entry;if(legacy)permanentRedirect(new URL(entry.permanent_url).pathname);const query=new URLSearchParams({drive:entry.org_id});if(entry.type==="folder")query.set("path",entry.path);else query.set("entry",entry.id);redirect(`/files?${query}`);}
 const entry=await response.json() as PublicEntry;
 return <Providers session={{signedIn:false,account:null,expiresAt:null}}><PublicViewer entry={entry} owner={owner}/></Providers>;
}
