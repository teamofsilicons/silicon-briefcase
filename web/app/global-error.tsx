"use client";

/**
 * The last resort: the root layout itself failed, so this renders its own <html> with the tokens inline (the
 * stylesheets may be what failed). Plain words and one way back.
 */
export default function GlobalError({ error, reset }: { error: Error & { digest?: string }; reset: () => void }) {
  return (
    <html lang="en">
      <body style={{ margin: 0, minHeight: "100vh", display: "grid", placeItems: "center", background: "#FCFCFC", color: "#1B1B1B", font: "16px/1.4 system-ui, -apple-system, BlinkMacSystemFont, \"Segoe UI\", sans-serif" }}>
        <main style={{ maxWidth: 460, padding: 24, textAlign: "center" }}>
          <h1 style={{ margin: "0 0 12px", fontSize: 28, fontWeight: 600, letterSpacing: "-0.03em" }}>This site stopped working</h1>
          <p style={{ margin: "0 0 20px", color: "#505050" }}>Something failed before the page could load. Try again in a moment.{error.digest ? ` Reference ${error.digest}.` : ""}</p>
          <button type="button" onClick={reset} style={{ font: "inherit", padding: "8px 16px", borderRadius: 6, border: 0, background: "#181818", color: "#FCFCFC", fontWeight: 500, cursor: "pointer" }}>Try Again</button>
        </main>
      </body>
    </html>
  );
}
