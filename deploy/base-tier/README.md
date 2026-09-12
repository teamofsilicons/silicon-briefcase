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
and restart the browser service. Browser restarts end in-memory browser sessions;
users sign in again through IAM.

The container stays read-only except for its private staging volume and small
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
Store releases beneath `/var/www/briefcase-docs/releases/<commit>` and atomically
replace the `current` symlink after verifying `index.html` and `openapi.yaml`.

Point `docs.briefcase.teamofsilicons.com` at the host's Elastic IP with
`deploy/dns.sh --host docs.briefcase.teamofsilicons.com --type A --value <ip> --apply`.
The DNS helper preserves the existing zone and lets explicit arguments override
the deployment configuration. Establish an HTTP ACME webroot at `/var/www/certbot`,
issue a dedicated Certbot certificate, then install `docs.nginx.conf` as
`/etc/nginx/conf.d/briefcase-docs.conf`. Run `nginx -t` before reloading.
The docs use directory indexes and their own 404 page. Rollback changes only
the `current` symlink to the preceding release.
