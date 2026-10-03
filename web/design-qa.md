# Briefcase light redesign — design QA

> Historical report preserved from the primary checkout’s uncommitted worktable design. Its evidence and results describe 1 October, not this migration run. See `design-qa-iam5.md` for the current integration and validation.

Reviewed 1 October 2026. **final result: passed**

**Findings**

No actionable P0, P1, or P2 findings remain in the final desktop and mobile evidence. The implementation preserves the chosen direction: a quiet light worktable, neutral typography, soft file wells, pill controls, restrained blue selection, and contextual file actions. This is a creative implementation of the Bencho philosophy, as requested, rather than a pixel clone or a replacement of real file contents with mock artwork.

- [P3] Unify the wordmark's optical treatment in a later polish pass. The sign-in wordmark is lighter, slightly larger, and has a blue briefcase icon; the workspace wordmark is heavier with a neutral icon. Both are clear and consistent with their surrounding hierarchy. Sharing the same wordmark component would remove this minor variation. Evidence: `implementation-signin.jpg` and the focused header comparison.

**Visual truth and rendered evidence**

Source visual truth:
`/Users/codanium/.codex/generated_images/01a0f3fb-a86b-7001-a4ae-388f873ca5a7/exec-3da3403e-1536-4bda-ad4f-47c4f14cab70.png`

The evidence directory is:
`/Users/codanium/Documents/silicon/.design/briefcase-2026-10-01/`

Final desktop rendering:
`/Users/codanium/Documents/silicon/.design/briefcase-2026-10-01/implementation-desktop-final.jpg`

Final mobile rendering:
`/Users/codanium/Documents/silicon/.design/briefcase-2026-10-01/implementation-mobile-final.jpg`

Additional opened evidence: `implementation-mobile-menu.jpg`, `implementation-signin.jpg`, `implementation-tablet.jpg`, `implementation-first.jpg`, `implementation-desktop.jpg`, and `implementation-mobile-footer-before.jpg`.

| Artifact | Pixel dimensions | CSS viewport / normalization |
| --- | --- | --- |
| Source generated direction | 1487 × 1058 | Raster visual truth; no browser CSS viewport or device frame |
| Final desktop | 1487 × 1058 | Captured at 1487 × 1058; 1 capture pixel per CSS pixel |
| Final mobile | 390 × 844 | Captured at 390 × 844; document width verified as 390 by the lead agent |
| Tablet | 820 × 1180 | Captured at 820 × 1180; document width verified as 820 by the lead agent |
| Mobile Add menu | 390 × 844 | Same mobile viewport; earlier capture before footer refinement |
| Sign-in | 1487 × 1058 | Same desktop viewport |
| Initial implementation | 1440 × 1024 | Historical evidence only; not treated as a pixel-aligned full-view comparison |

No density resampling was required for the final source/render pair. Full and focused comparisons place both artifacts in the same image at their original scale, with labels outside the captured content. Image previews may be downscaled by the viewer; focused crops preserve readable details.

Desktop state: light mode, local `design-preview` organization, `Design / Autumn campaign`, grid view, Brand guide selected. The source also depicts an upload in progress, while the final implementation is idle. Comparison therefore judges the common header, file grid, selected card, action dock, and footer; it does not demand a permanent upload indicator in an idle workspace.

**Combined comparison evidence**

- `qa-comparison-full.png`: source and final desktop side by side, both 1487 × 1058.
- `qa-comparison-header.png`: matched 995 × 315 crops, showing identity, search, breadcrumb, title, and navigation typography.
- `qa-comparison-selected-card.png`: matched 402 × 439 crops, showing selected file, metadata, and action dock.
- `qa-search-iteration.png`: before/after search control crops, at native capture scale.
- `qa-footer-iteration.png`: matched desktop bottom regions before/after the persistent footer fix.
- `qa-mobile-states.png`: original mobile files and Add menu, side by side.
- `qa-mobile-footer-iteration.png`: final mobile comparison against the earlier two-row sticky footer, both 390 × 844.

All combined images above were opened and inspected. The final mobile footer comparison confirms that the first card's filename and metadata remain visible while Bin and Organisation settings are accessible.

The additional tablet capture was opened independently. Its three-column shelf, selected action dock, full-width search/header arrangement, and persistent footer remain clear. The lead agent measured identity at x30–332 and search at x350–670; the screenshot confirms their separation and no horizontal clipping at 820px.

**Required fidelity surfaces**

| Surface | Assessment |
| --- | --- |
| Fonts and typography | Neutral sans-serif maintains the source's plain, precise character. Arial/Helvetica/system fallback is intentional; the generated source does not identify a licensed font family. The implementation uses a slightly quieter display weight and smaller metadata than the mock. Focused crops show legible hierarchy, clean baselines, sensible filename truncation, and no cramped labels. Mobile title wraps into two deliberate lines without colliding with Add files. The small wordmark difference is the P3 item above. |
| Spacing and layout rhythm | Desktop preserves four equal file columns, generous margins, soft 24–28px wells, compact metadata, and a selected-file dock. Folder ordering no longer creates the initial large holes. Header/search alignment and the shared selected-card region are close to the target's proportions. Mobile intentionally becomes one column; search occupies a separate row and navigation wraps cleanly. The final compact footer uses one row. |
| Colors and tokens | Light canvas `#fafaf8`, wells `#f0f0ec`, ink `#20221f`, white controls, and exact accent `#1F5FB8` implement the requested palette. Small text uses `#696d65`, giving calculated contrast of 4.62:1 on wells and 5.05:1 on the canvas. White on the primary blue is 6.20:1. Selection and destructive/lifetime states retain semantic distinction. No dark surface appears in reviewed states. |
| Image quality and assets | The campaign cover is a real sample PNG served as file content; its architectural subject, light, and quiet palette fit the direction. Crop and edges are clean. Actual readable image files supply thumbnails. Documents and video use functional library file-type icons and real filenames because the backend provides no general document/video thumbnail service. These are explicit file representations, not fabricated previews of their contents. The source's branded document cover, landscape video poster, avatar photo, and exact architectural image are mock content and are not substituted for users' files. There are no invented decorative illustrations in sign-in. |
| Copy and content | Controls use task language: Add files, Public, Private, Preview, Bin, Organisation settings, and readable recipient types. Real names, extensions, byte sizes, dates, organization IDs, and breadcrumb depth come from available data. Copy avoids claiming unavailable folder counts, profile photos, or public permissions. The source's running upload, team subtitle, Design tag pill, exact dates, and item counts are not falsely presented when the local fixture does not supply equivalent state. |

**Comparison history and resolved findings**

1. **Initial visual pass — blocked.** The initial 1440 × 1024 capture showed a nested white text field inside the search pill, folder-first ordering that created large grid gaps, and generic file icons where the fixture's image/render classification was incorrect. These were moderate (P2) clarity/density issues. The search surface was unified, fixture ordering/render values corrected, and real sample image content supplied. Final evidence: `qa-search-iteration.png`, `qa-comparison-full.png`, and `qa-comparison-selected-card.png`.
2. **Desktop same-viewport pass — blocked.** At 1487 × 1058, the main file view looked coherent but Bin/settings fell below the captured viewport. The footer became sticky and storage details moved into the account menu. Final evidence: `qa-footer-iteration.png` and `implementation-desktop-final.jpg`, with both persistent controls visible.
3. **Mobile footer refinement — blocked until recapture.** The first sticky footer used two rows and covered the first file's metadata. The desktop drag hint was hidden at touch widths, reducing the footer from 89px to 60px. Final evidence: `qa-mobile-footer-iteration.png` and `implementation-mobile-final.jpg`. Filename, metadata, Bin, and Organisation settings are visible; document width remains 390px.
4. **Final independent comparison — passed.** Source and final implementation were reviewed in a combined full-view image and focused header/selected-card crops. The current mobile footer was then compared directly against its preceding state. No actionable P0/P1/P2 remains in these reviewed surfaces.

The lead agent also corrected the squared Download anchor and raw recipient-type labels during its broader UI checks. These were reported refinements outside the common source screenshot's visible state; this independent screenshot review does not claim new visual evidence for those transient dialogs.

**Interaction validation and practical limits**

The lead agent reports successful browser exercise of grid/list switching, protected Markdown iframe preview, search/back, create/rename, a single-picker move flow, sharing with a one-day expiry, bin/restore, multiple uploads, lost-response recovery with the same operation identity, explicit 413 dismissal, upload destination selection, organization-root reload, and simulated login/logout. It reports an empty console error list (`[]`), passing lint/typecheck/build, and seven passing automated tests (five local gateway tests and two telemetry tests). Those checks are separate from this independent image comparison and were not rerun by this reviewer.

The earlier code review found and the lead agent fixed silent upload loss on workspace switch, unrecoverable retry handling for expired-session/definitive-conflict responses, and the empty organization-root public-view routing bug. The lead agent added navigation warning behavior for active uploads. Production IAM, deployed backend permissions, large production files, and physical mobile browsers remain outside this local fixture proof.

Keyboard focus styles, native semantic buttons, named icon controls, reduced-motion handling, light-only tokens, and contrast were inspected in code during the preceding implementation review. The supplied screenshots do not establish full screen-reader behavior, 200% text zoom, every modal state, or every intermediate breakpoint. These are residual coverage limits, not observed blocking defects.

**Open questions and expected differences**

No unresolved product decision blocks this handoff. The user delegated direction choice and required all-light presentation. The source is treated as broad art direction; dynamic file content, identity initials, timestamps, permissions, navigation availability, and upload state follow actual data. A future authenticated thumbnail service could provide richer document/video covers without using mock artwork, but it is not required for a truthful, usable file browser.

**Implementation checklist**

- [x] Compare source and final desktop in the same image.
- [x] Inspect focused header and selected-card regions at native scale.
- [x] Check typography, spacing, color, assets, and app copy explicitly.
- [x] Verify desktop and mobile persistent controls after their fixes.
- [x] Inspect mobile Add menu and desktop sign-in.
- [x] Inspect the 820px tablet breakpoint and confirm no observed overlap.
- [x] Distinguish local interaction evidence from production integration proof.
- [ ] Optional P3: share one wordmark treatment between entry and workspace.

**final result: passed**
