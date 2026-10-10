# Silicon UI provenance

Official registry: https://ui.teamofsilicons.com/r/{name}.json. Usage documentation:
https://ui.teamofsilicons.com/llms.txt and https://ui.teamofsilicons.com/button.md.
The source is https://github.com/teamofsilicons/silicon-ui, under the included MIT
license retaining Team of Silicons and original Arc attribution.

The actual component source was fetched from the official registry on 2026-10-10,
including foundation, motion tokens and the transitive shared menu helper. The
registry lock records SHA-256 of each response and unmodified source file. The selected set covers Briefcase and Remind controls with their shared dependencies.

Local integration: Dialog and Drawer keep the application's existing opener-focus
restoration and nested-Escape behavior through lib/return-focus.ts and
lib/layer-escape.ts. Brand tokens, server-only sign-in and app surfaces remain in
the application. The Dialog and Drawer integrations are deliberate local edits;
the primary button maps its color to the existing brand tokens. Other imported
registry source is unchanged except for the accessible account label below.

The user menu marks decorative initials as aria-hidden and includes the visible initials alongside the full account name in the trigger label. This preserves label-in-name accessibility for multi-word display names and voice control.

The hold-to-confirm ink overlay is hidden while idle (its clip is empty); this avoids duplicate visible-label detection before a hold begins.
