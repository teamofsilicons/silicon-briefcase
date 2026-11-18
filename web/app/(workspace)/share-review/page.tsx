import { ShareReview } from "@/components/briefcase/share-review";
import { ErrorAlert } from "@/components/foundation/feedback/error-alert";
import type { Drive } from "@/lib/briefcase/types";
import { tryApiFetch } from "@/lib/server/rsc";

export default async function ShareReviewPage({ searchParams }: { searchParams: Promise<{ drive?: string }> }) {
  const [drives, params] = await Promise.all([tryApiFetch<{ items: Drive[] }>("/v1/drives"), searchParams]);
  return drives.error ? <ErrorAlert error={drives.error} /> : <ShareReview drives={drives.data.items} initialDrive={params.drive} />;
}
