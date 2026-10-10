import { Settings } from "@/components/briefcase/settings";
import { tryApiFetch } from "@/lib/server/rsc";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import type { Drive } from "@/lib/briefcase/types";
export default async function SettingsPage() { const result = await tryApiFetch<{ items: Drive[] }>("/v1/drives"); return result.error ? <ErrorAlert error={result.error} /> : <Settings initialDrives={result.data} />; }
