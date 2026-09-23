# Briefcase 2.0.0 public identifier candidate

This candidate changes public application IDs to bare handles and actor IDs to `c:` / `si:` handles, retaining the existing `/api/v1` route namespace. Backend, Rust client, CLI, browser gateway, and contract document are version 2.0.0. Identity-bearing operation revisions are synchronized in the service registry, client registry, OpenAPI, and operation inventory. Deploy the matching contract and consumers together.

The source preserves the deployed backend changes from 5247cc1 and browser gateway changes from e33e20a. Source parity was checked before staging. The release pipeline pins Honeycomb packager b39b1ceb96997ec4c80cb5c5354130ecb9da73eb, which accepts the bare `briefcase` manifest. The candidate workflows produce six native CLI artifacts and ARM64 backend, gateway, and documentation image archives without publication or deployment.

## Migration and recovery gates

Follow `docs/identifier-migration.md` and the cross-service migration mapping. Migration 0046 and the offline migration preserve private UUID bindings, object bytes, encryption material, and prior public paths. Current production and testing queues must pass the fail-closed guards. Stop writers and take fresh matched production backups at the coordinated cutover; this pre-clean testing snapshot does not replace the production cutover backup.

The running topology is the existing base-tier ARM64 host i-00c2b0c4968b57186 in us-east-1, with separate API, worker, and gateway systemd units. Use this topology, preserving environment files, RDS CA mount, telemetry mounts, and private staging/session volumes. Do not run the historical ASG deployment helper.

The RDS instances run PostgreSQL 17.9; the host tools are PostgreSQL 15.19. Stage PostgreSQL 17-compatible dump/restore tools or a controlled client container before running the migration backup command.

## Verified testing reset backup

Before the authorized testing reset, the encrypted manual snapshot `arn:aws:rds:us-east-1:234951665042:snapshot:briefcase-public-identifiers-test-preclean-20260923` was verified available, 100 percent complete, from `silicon-briefcase-testing-production`, created 2026-09-23T11:20:40.370Z.

The matched host configuration and key files are archived in `/var/lib/silicon-briefcase/backups/public-identifiers-test-preclean-20260923/protected-config.tar.gz`, with SHA-256 `eec269db16376589d75a88c1e06f2f2f2bbd4afb8c8dfca07e81e59c32856a3e`. All 24 archived files were verified against their original hashes. The directory is mode 0700 and archive/manifest mode 0600, owned by root. No secrets were included in command output. Backup SSM command: `3f3efd21-a17e-45f5-8192-afbf5eba48b9`.

## Local candidate validation

- Backend library: 241 passed, one ignored; strict all-target/all-feature Clippy passed.
- Client, CLI, and gateway workspace: 136 passed, two ignored; strict workspace all-target/all-feature Clippy passed.
- Standalone client 2.0.0 Cargo package verification passed.
- Four real PostgreSQL 16 migration tests passed, covering previews, rollback, collisions, preserved identity keys/content, path aliases, and testing-plane isolation.
- Installer sequencing: three passed; no updater daemon is started automatically.
- Documentation: 18 pages and 823 local links/assets checked. Web production build passed.
- Bare `briefcase` version 2.0.0 manifest validated, packed, and revalidated with Honeycomb 0.4.0 using structural six-target fixtures. Compiled native release evidence comes from the build-only workflow, not these fixtures.

The testing snapshot and protected host archive were the only infrastructure mutations performed during this preflight. Candidate source/artifact staging does not publish crates, change latest pointers, or restart production services.
