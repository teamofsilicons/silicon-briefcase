"use client";

/**
 * A page crashed while rendering: say so plainly, keep the digest for a report, and offer to try again.
 */
import { useEffect } from "react";
import { RotateCcw } from "lucide-react";
import { Button } from "@teamofsilicons/silicon-ui/button";
import { ButtonLink } from "@teamofsilicons/silicon-ui/button-link";
import { Problem } from "@/components/foundation/feedback/problem";

export default function ErrorPage({ error, reset }: { error: Error & { digest?: string }; reset: () => void }) {
  useEffect(() => {
    console.error(error);
  }, [error]);
  return (
    <main id="main">
      <Problem
        icon={<RotateCcw strokeWidth={1.5} />}
        title="This page stopped working"
        reference={error.digest}
        actions={
          <>
            <Button variant="secondary" onClick={reset}>Try Again</Button>
            {/* A plain <a>, so a full load: the page that failed may have broken the client router. */}
            <ButtonLink href="/" variant="ghost">Go to the Home Page</ButtonLink>
          </>
        }
      >
        <p>Something in the page failed{error.digest ? "" : `: ${error.message || "an unexpected error"}`}. Try again; if it keeps happening, quote the reference below when you report it.</p>
      </Problem>
    </main>
  );
}
