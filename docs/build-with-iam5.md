# Build a file workflow that respects the user's workspace

Briefcase should feel like a useful part of your app: the report arrives where the user expects, an interrupted upload can resume, and storage access is requested when it is needed.

**Available now:** Briefcase contract 3.0.0, the published Rust SDK and CLI 3.0.1, IAM 5 reusable OBO verification, and Briefcase's account picker and Carbon/Silicon popup interface. The integration pattern below is guidance for your app; each consuming application still needs its own implementation and end-to-end verification.

## Make the account choice explicit

Offer **Continue as Carbon** and **Continue as Silicon**. Each choice opens IAM in a popup with `identity_kind=carbon` or `identity_kind=silicon`, `display=popup`, your canonical application ID, and your registered callback URL. Preserve a random, single-use correlation state on your backend together with the selected kind. Exchange the returned SLT on the backend and verify the authenticated actor type before saving a session.

A successful login represents one account and one organization. To support several workspaces, retain a separate session for each account–organization pair; never change an organization header on an existing bearer. Bind pending requests and response rendering to the workspace that initiated them, including its testing environment. An account switch must not display an older workspace's delayed results or repeat its mutations.

For popup completion, check the exact opener window, exact origin and saved nonce. Send only a completion signal, then load your own authenticated session again. Do not put access tokens, refresh tokens or app secrets in messages, local storage, URLs or logs. A blocked or closed popup should leave an understandable retry action.

## Ask when the feature needs access

Login and OBO approval are separate. Request only the endpoint graph needed for the feature the person or silicon is using. IAM shows the requested endpoints, dependencies, warnings and the account–organization destination for each provider. An organization selected for a provider may differ from the app's login workspace; use the verified provider destination, while retaining your own request ownership and resource checks.

For a browser flow, supply a fixed registered `redirect_uri` and unpredictable `state` when initiating the IAM OBO authorization, and open its authorization URL with `display=popup`. Validate the callback against the original account, organization, environment and request before exchanging its one-use code. A CLI can omit the callback and use the manual code completion flow. Never make successful approval silently repeat a paid operation or mutation.

Retain the exact pending operation, code and exchange retry identity after an uncertain response. A retry should finish that operation rather than ask for another grant. Keep resulting OBO credentials on the backend, refresh their separate token family when needed, and verify current authority at the receiving endpoint. Revocation must stop future use. ATA is application authority and cannot be passed to OBO routes or converted into user authority later in a chain.

For the authoritative login, OBO and ATA contracts, use [IAM documentation](https://docs.iam.teamofsilicons.com/). For the full publication journey, read [Making a Team of Silicons ready application](https://docs.honeycomb.teamofsilicons.com/guides/team-of-silicons-ready-applications/). Configure application authority through [centralized ATA verifications](https://docs.honeycomb.teamofsilicons.com/app-to-app/) and the [IAM ATA client contract](https://docs.iam.teamofsilicons.com/client/ata/).

## Give the CLI the complete workflow

Support login, selecting a saved account and organization, requesting feature consent, uploading, inspecting progress and recovering uncertain results from the CLI. The website can use the same operations through its backend. Keep machine-readable errors and explicit retry identifiers so a silicon can recover without interpreting a screenshot.

## Upload safely, then publish once

Register and request the needed Briefcase endpoints in Honeycomb. For uploads, use `briefcase.uploads.reserve`, `briefcase.uploads.commit`, and `briefcase.uploads.status`; add cancellation only if your app supports it. Reserve private staging, transfer the bytes with the returned capability, and commit with a currently valid OBO token. The capability can stage bytes but cannot publish content. The retired raw delegated upload returns 410.

The receiving routes use `X-App-ID` plus `X-IAM-OBO-Access-Token`. Keep exact operation bytes and the logical operation ID for retries; a reusable token does not make a mutation safe to duplicate. Briefcase still checks the verified destination's visibility, permission and quota. See [delegated uploads](api/delegated-uploads.md), [OBO endpoint reference](obo.md), and [Rust client](client/README.md).

## Prove it before publishing

Use [testing environments](testing-environments.md) to exercise both identity kinds, two accounts with the same organization name, another provider destination, revoked consent and interrupted reserve/commit responses. Keep testing credentials and data isolated from production. A sandbox success does not prove production storage configuration; verify the deployed service version and your actual registered endpoint graph before publication.

- [ ] Carbon and Silicon can finish the workflow through the CLI.
- [ ] Each account and organization keeps its own session and pending work.
- [ ] Feature approval is separate from login and cancellation leaves work untouched.
- [ ] Upload retries keep the same operation identity and reconcile status.
- [ ] Revoked and uncertain authorization fail without publishing content.
- [ ] Testing, provider review, documentation and the Honeycomb release are complete.
