import { sites } from '@openai/sites-vite-plugin';
import tailwindcss from '@tailwindcss/postcss';
import vinext from 'vinext';
import { defineConfig, type ProxyOptions, type ViteDevServer } from 'vite';
import hostingConfig from './.openai/hosting.json' with { type: 'json' };

const SITE_CREATOR_PLACEHOLDER_DATABASE_ID =
  '00000000-0000-4000-8000-000000000000';

const { d1, r2 } = hostingConfig;

const browserProxy: ProxyOptions = {
  target: 'http://127.0.0.1:4318',
  changeOrigin: false,
  configure(proxy) {
    proxy.on('proxyReq', (outgoing, incoming) => {
      if (
        incoming.method === 'POST' &&
        incoming.url?.split('?', 1)[0] === '/browser/upload'
      ) {
        // Let the gateway perform admission before the first body chunk.
        // Node otherwise holds headers until data arrives or the body ends.
        outgoing.flushHeaders();
      }
    });
  },
};

// macOS Seatbelt blocks FSEvents, so Codex previews need polling for HMR.
const isCodexSeatbeltSandbox = process.env.CODEX_SANDBOX === 'seatbelt';

const localBindingConfig = {
  main: 'vinext/server/fetch-handler',
  compatibility_flags: ['nodejs_compat'],
  d1_databases: d1
    ? [
        {
          binding: d1,
          database_name: 'site-creator-d1',
          database_id: SITE_CREATOR_PLACEHOLDER_DATABASE_ID,
        },
      ]
    : [],
  r2_buckets: r2
    ? [
        {
          binding: r2,
          bucket_name: 'site-creator-r2',
        },
      ]
    : [],
};

export default defineConfig(async () => {
  // Keep Wrangler and Miniflare state project-local. These are non-secret tool
  // settings; application environment belongs in ignored `.env*` files.
  process.env.WRANGLER_WRITE_LOGS ??= 'false';
  process.env.WRANGLER_LOG_PATH ??= '.wrangler/logs';
  process.env.MINIFLARE_REGISTRY_PATH ??= '.wrangler/registry';

  // Wrangler snapshots its log path while the Cloudflare plugin is imported.
  const { cloudflare } = await import('@cloudflare/vite-plugin');

  return {
    css: { postcss: { plugins: [tailwindcss()] } },
    server: {
      proxy: {
        '/browser': browserProxy,
        '/auth/callback': {
          target: 'http://127.0.0.1:4318',
          changeOrigin: false,
        },
      },
      ...(isCodexSeatbeltSandbox
        ? { watch: { useFsEvents: false, usePolling: true } }
        : {}),
    },
    plugins: [
      {
        name: 'briefcase-file-links',
        configureServer(server: ViteDevServer) {
          server.middlewares.use((request, _response, next) => {
            // The exported app is a single workspace shell; the browser reads
            // the untouched address bar and resolves the target through the SDK.
            if (
              (request.method === 'GET' || request.method === 'HEAD') &&
              request.url?.startsWith('/org/')
            ) {
              request.url =
                '/' +
                (request.url.includes('?')
                  ? '?' + request.url.split('?').slice(1).join('?')
                  : '');
              // Cloudflare's local adapter restores originalUrl before dispatch.
              request.originalUrl = request.url;
            }
            next();
          });
        },
      },
      vinext(),
      sites(),
      cloudflare({
        viteEnvironment: { name: 'rsc', childEnvironments: ['ssr'] },
        config: localBindingConfig,
      }),
    ],
  };
});
