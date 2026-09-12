# Briefcase documentation site

Build and verify:

```sh
cd docs-site
npm ci
npm run build
npm run check
```

Serve `dist/` at **https://docs.briefcase.teamofsilicons.com**. Routes use directory indexes, so `/api/` serves `api/index.html`; do not apply an SPA rewrite. Set a custom 404 response to `404.html`. Canonical URLs, sitemap, robots and search use the production docs host. Source Markdown lives in `../docs`; `../openapi.yaml` is copied to `/openapi.yaml`.

For a local preview, run `python3 -m http.server 4400 --directory dist`. No API credentials or application secrets are needed. Generated output and node_modules are ignored. CI builds and checks every local link and fragment. Deploy the build output through the hosting provider configured for this hostname; this repository does not assume or modify DNS or publish automatically.
