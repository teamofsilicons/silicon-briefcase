import { FileBrowser } from "@/components/briefcase/file-browser";
import { requireSession, tryApiFetch } from "@/lib/server/rsc";
import type { Drive, Entry } from "@/lib/briefcase/types";
export default async function FilesPage({ searchParams }: { searchParams: Promise<{ drive?: string; path?: string; entry?: string }> }) {
  const params = await searchParams;
  const session = await requireSession();
  const drives = await tryApiFetch<{ items: Drive[] }>("/v1/drives");
  const entry = params.entry ? await tryApiFetch<Entry>(`/v1/entries/${encodeURIComponent(params.entry)}`) : null;
  return <FileBrowser mode="shared" accountUuid={session.account.uuid} drive={params.drive} folder={params.path} initialDrives={drives.data ?? undefined} initialEntry={entry?.data ?? undefined} />;
}
