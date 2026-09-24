# Briefcase base-tier hosting

The current production topology is a single ARM64 EC2 instance with Nginx TLS
termination and systemd-managed API, worker, and browser containers. There is no
load balancer or autoscaling refresh in this deployment path. The older
CloudFormation/ASG scripts are retained for infrastructure history and must not
be used to update the base-tier host.

## Ingress and uploads

Install `backend.nginx.conf` as `/etc/nginx/conf.d/base-tier.conf` and
`browser.nginx.conf` as `/etc/nginx/conf.d/briefcase-web.conf`. Existing Certbot
certificates remain at the paths in these files. Back up both configurations,
run `nginx -t`, and reload Nginx only after validation succeeds. A reload keeps
existing connections alive.

Only the normal raw upload, one-request OBO upload, delegated content transfer,
and browser upload locations receive the 5 TiB application ceiling. JSON and
webhook routes retain their lower limits. Raw uploads disable Nginx request
buffering so the proxy does not stage large bodies on its own disk. Their
900-second upstream read/write timeouts match the API upload deadline.

## Browser temporary storage

Create `/var/lib/silicon-briefcase/web-staging` owned by `65532:65532`, mode
`0700`. Install `web-upload.conf` as the systemd drop-in
`/etc/systemd/system/silicon-briefcase-web.service.d/upload.conf`; update its
image reference together with each browser release. Run `systemctl daemon-reload`
and restart the browser service.

Create `/var/lib/silicon-briefcase/web-sessions` owned by `65532:65532`, mode
`0700`, for the persistent `BRIEFCASE_WEB_SESSION_DIRECTORY` volume in the same
drop-in. It contains private IAM credentials and must be retained across image
updates. The gateway takes an exclusive process lock and verifies the saved API
and browser origin before loading sessions; run one gateway against this directory.
Sessions, pending login callbacks, refresh retries, and logout state survive
restarts. The first upgrade from the old memory-only gateway requires one new
sign-in because its old sessions were never stored. Local development defaults to
`.briefcase-web-sessions` in the working directory.

The container stays read-only except for its private staging and session volumes and small
temporary mount. The gateway checks available filesystem space, reserves an
additional 1 GiB of headroom, accounts for concurrent uploads, and removes staged
files after use. Provision disk capacity for the intended upload workload;
the 5 TiB protocol ceiling does not promise that a base-tier host can stage a
5 TiB file. Files remain private until the backend authorizes and publishes them.

## Updating application images

Build the frontend assets before building `Dockerfile.browser`; build both
images for `linux/arm64`, then push immutable release tags to ECR. Pull the
images on the host before changing service references. The API and worker use
the same backend image. The browser's effective image reference is in its
systemd drop-in, not just the base unit. Preserve the env files and their
credentials, IAM bindings, and mounted RDS CA bundle.

Apply migrations separately to both production and testing databases with each
database's migrator credential. Neither runtime role should acquire migration
permissions. Restart only changed services, check all three services and
container images, verify `/readyz`, then sign in and perform a real upload/read.
Keep previous image tags and unit/configuration backups available for rollback.
Do not roll database migrations backward as an application rollback step.

## Documentation

Build and check `docs-site`, then build `Dockerfile.docs` from the repository
root. This scratch image packages only `/docs`; it is an immutable content
artifact and is never run. Pin it by its ECR digest, create a stopped container,
and use `docker cp <container>:/docs/. <release-directory>` to extract it.
Store releases beneath `/var/www/briefcase-docs/releases/<commit>`. After extraction,
explicitly run `chmod 0755 <release-directory>` so Nginx can traverse the release
even when the deployment shell uses `umask 077`. Check that nested directories are
traversable and static files are readable by Nginx, then verify `index.html` and
`openapi.yaml` before atomically replacing the `current` symlink. Check the public
docs page and OpenAPI URL after switching; HTTP 403 can indicate a directory-mode
error even when the extracted content and symlink are correct.

Point `docs.briefcase.teamofsilicons.com` at the host's Elastic IP with
`deploy/dns.sh --host docs.briefcase.teamofsilicons.com --type A --value <ip> --apply`.
The DNS helper preserves the existing zone and lets explicit arguments override
the deployment configuration. Establish an HTTP ACME webroot at `/var/www/certbot`,
issue a dedicated Certbot certificate, then install `docs.nginx.conf` as
`/etc/nginx/conf.d/briefcase-docs.conf`. Run `nginx -t` before reloading.
The docs use directory indexes and their own 404 page. Rollback changes only
the `current` symlink to the preceding release.

## Telemetry

Store `BRIEFCASE_TELEMETRY_TABLE_KEY` for `tos/siliconbriefcase` in the existing
application secret in Secrets Manager. Preserve all other secret fields.
Copy only the telemetry settings into the API and worker's mode-0600 runtime
env files. The browser receives no table key. Use
`BRIEFCASE_TELEMETRY_URL=https://backend.spacestation.teamofsilicons.com` and
`BRIEFCASE_TELEMETRY_HOME=/var/lib/silicon-briefcase/telemetry`.

Create separate host directories `/var/lib/silicon-briefcase/telemetry-api` and
`/var/lib/silicon-briefcase/telemetry-worker`, owned by `10001:10001`, mode 0700.
Mount the appropriate directory at `/var/lib/silicon-briefcase/telemetry` in each
service's Docker ExecStart. Keep existing staging and CA mounts. Back up the
units and env files, run `systemctl daemon-reload`, and restart with the new
image. `BRIEFCASE_TELEMETRY=off` is the operator override.

After release, verify `/api/version` exposes `submitTelemetry`, submit a labelled
content-free test event, and confirm its UUID in the Space Station table. Also
verify the browser telemetry setting and the public CLI installer/docs.
