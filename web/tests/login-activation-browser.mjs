// Actual React Home/API components in Chromium, with synthetic loopback HTTP.
// This proves browser wiring and selectors; Rust gateway tests prove cookie
// authentication, group membership, and durable cancellation on the server.
// PLAYWRIGHT_MODULE=/absolute/path/to/@playwright/test/index.mjs node tests/login-activation-browser.mjs
import assert from 'node:assert/strict';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { createServer } from 'vite';
import react from '@vitejs/plugin-react';

if (!process.env.PLAYWRIGHT_MODULE)
  throw new Error(
    'Set PLAYWRIGHT_MODULE to an existing local Playwright module.',
  );
const { chromium, expect } = await import(
  pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
);
const root = fileURLToPath(new URL('..', import.meta.url));
const html =
  '<!doctype html><html><body><div id="root"></div><script type="module" src="/fixture-entry.jsx"></script></body></html>';
const source = `import React from 'react';import {createRoot} from 'react-dom/client';import Home from '/app/page.tsx';createRoot(document.getElementById('root')).render(React.createElement(Home));`;
const server = await createServer({
  root,
  configFile: false,
  logLevel: 'error',
  define: { 'process.env': '{}' },
  resolve: {
    alias: {
      '@': root,
      'next/link': fileURLToPath(
        new URL('../node_modules/vinext/dist/shims/link.js', import.meta.url),
      ),
    },
  },
  plugins: [
    react(),
    {
      name: 'briefcase-local-home-fixture',
      configureServer(vite) {
        vite.middlewares.use(async (req, res, next) => {
          const path = req.url.split('?')[0];
          if (path === '/' || path.startsWith('/org/')) {
            res.writeHead(200, { 'content-type': 'text/html' });
            res.end(await vite.transformIndexHtml(req.url, html));
            return;
          }
          next();
        });
      },
      resolveId(id) {
        if (id === '/fixture-entry.jsx') return '\0fixture-entry.jsx';
      },
      load(id) {
        if (id === '\0fixture-entry.jsx') return source;
      },
    },
  ],
  server: { host: '127.0.0.1', port: 0 },
});
await server.listen();
const origin = `http://127.0.0.1:${server.httpServer.address().port}`;
const A = '11111111-1111-4111-8111-111111111111';
const B = '22222222-2222-4222-8222-222222222222';
const C = '33333333-3333-4333-8333-333333333333';
const UNKNOWN = '44444444-4444-4444-8444-444444444444';
const EXPIRED = '55555555-5555-4555-8555-555555555555';
const records = [
  {
    context_id: A,
    org: 'proof-a',
    actor: { type: 'carbon', public_id: 'c:fixture' },
  },
  {
    context_id: B,
    org: 'proof-b',
    actor: { type: 'silicon', public_id: 'si:fixture' },
  },
  {
    context_id: C,
    org: 'proof-c',
    actor: { type: 'carbon', public_id: 'c:other' },
  },
];
const session = (id) => {
  const found = records.find((item) => item.context_id === id);
  return found
    ? {
        authenticated: true,
        ...found,
        contexts: records,
        organizations: [found.org],
        testing: false,
      }
    : {
        authenticated: false,
        context_id: null,
        contexts: [],
        organizations: [],
        org: null,
        testing: false,
      };
};
const deferred = () => {
  let resolve;
  const promise = new Promise((done) => {
    resolve = done;
  });
  return { promise, resolve };
};
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext();
const pageErrors = [],
  requests = [],
  starts = [],
  cancellations = [],
  activations = [];
const held = new Map();
let firstActivation, activationGate;
context.on('page', (page) =>
  page.on('pageerror', (error) => pageErrors.push(error.message)),
);
await context.addInitScript(() => {
  if (location.protocol === 'http:')
    localStorage.setItem('briefcase-telemetry', 'off');
});
await context.route('**/*', async (route) => {
  const request = route.request(),
    url = new URL(request.url());
  if (url.origin !== origin) return route.abort('blockedbyclient');
  if (url.pathname === '/fixture/iam')
    return route.fulfill({
      contentType: 'text/html',
      body: '<p>Local IAM handoff reached</p>',
    });
  if (!url.pathname.startsWith('/browser/')) return route.continue();
  const page = request.frame().page();
  const selector = request.headers()['x-briefcase-context'];
  assert.equal(url.searchParams.get('account_context'), selector ?? null);
  const body = request.postDataJSON();
  requests.push({
    page,
    path: url.pathname,
    selector,
    method: request.method(),
  });
  const reply = (value, status = 200, headers = {}) =>
    route.fulfill({
      status,
      contentType: 'application/json',
      headers,
      body: JSON.stringify(value),
    });
  if (url.pathname === '/browser/login/start') {
    assert.match(body.attempt_nonce, /^[a-f0-9]{64}$/);
    if (body.popup_nonce) assert.equal(body.popup_nonce, body.attempt_nonce);
    starts.push(body);
    const gate = deferred();
    held.set(body.attempt_nonce, gate);
    const status = await gate.promise;
    return reply(
      status === 200
        ? {
            redirect_url: `${origin}/fixture/iam?attempt=${body.attempt_nonce}`,
          }
        : { error: { message: 'This previous attempt was cancelled.' } },
      status,
    );
  }
  if (url.pathname === '/browser/login/cancel') {
    cancellations.push(body.attempt_nonce);
    return reply({ cancelled: true });
  }
  if (url.pathname === '/browser/login/activate') {
    activations.push(body);
    assert.equal(body.context_id, C);
    assert.equal(body.state, 'a'.repeat(64));
    if (firstActivation) {
      firstActivation.resolve();
      firstActivation = null;
      await activationGate.promise;
    }
    // Model a legacy or delayed shared selection cookie; each tab's public
    // selector must remain authoritative despite this browser-wide write.
    return reply({ ...session(C), return_to: '/org/proof-c/' }, 200, {
      'set-cookie': `fixture_shared_selection=${C}; Path=/; SameSite=Lax`,
    }).catch(() => {});
  }
  if (selector === UNKNOWN)
    return reply(
      { error: { message: 'Selected context is unavailable.' } },
      409,
    );
  if (url.pathname === '/browser/session') {
    if (selector === EXPIRED)
      return reply(
        { error: { message: 'The selected session expired.' } },
        401,
      );
    if (request.method() === 'PATCH') return reply(session(body.context_id));
    return reply(session(selector));
  }
  assert(
    records.some((item) => item.context_id === selector),
    `Unexpected unbound request ${url.pathname}`,
  );
  if (url.pathname === '/browser/entries')
    return reply({ items: [], next_cursor: null });
  if (url.pathname === '/browser/usage')
    return reply({
      storage: { used_bytes: 0, limit_bytes: 1000, remaining_bytes: 1000 },
      daily_uploads: { used_bytes: 0, limit_bytes: 1000 },
    });
  if (url.pathname === '/browser/notifications')
    return reply({ items: [], unread_count: 0 });
  throw new Error(`Unexpected fixture request: ${url.pathname}`);
});
const newPage = async (selector) => {
  const page = await context.newPage();
  await page.addInitScript((value) => {
    if (
      location.protocol === 'http:' &&
      !sessionStorage.getItem('briefcase-context:production')
    )
      sessionStorage.setItem('briefcase-context:production', value);
  }, selector);
  return page;
};
const selected = (page) =>
  page.evaluate(() => sessionStorage.getItem('briefcase-context:production'));
const sessionRequests = (page) =>
  requests.filter(
    (item) => item.page === page && item.path === '/browser/session',
  );
try {
  const page = await newPage('anonymous');
  await page.goto(origin);
  await page
    .getByRole('button', { name: 'Continue as Carbon', exact: true })
    .click();
  await expect.poll(() => starts.length).toBe(1);
  const old = starts[0].attempt_nonce;
  await expect(
    page.getByRole('button', {
      name: 'Continue as Silicon in this tab',
      exact: true,
    }),
  ).toBeEnabled();
  await page
    .getByRole('button', { name: 'Cancel sign-in', exact: true })
    .click();
  await expect.poll(() => cancellations.includes(old)).toBe(true);
  assert(
    held.has(old),
    'Cancellation must finish while the original start response is still held.',
  );
  await page
    .getByRole('button', {
      name: 'Continue as Silicon in this tab',
      exact: true,
    })
    .click();
  await expect.poll(() => starts.length).toBe(2);
  const next = starts[1].attempt_nonce;
  assert.notEqual(old, next);
  assert.equal(starts[1].identity_kind, 'silicon');
  assert.equal(starts[1].popup_nonce, undefined);
  held.get(old).resolve(409);
  await expect(
    page.getByRole('button', { name: 'Cancel sign-in', exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole('button', {
      name: 'Continue as Silicon in this tab',
      exact: true,
    }),
  ).toBeDisabled();
  assert.deepEqual(cancellations, [old]);
  held.get(next).resolve(200);
  await page.waitForURL(`${origin}/fixture/iam?attempt=${next}`);
  assert.deepEqual(
    cancellations,
    [old],
    'A late rejected start must not cancel its successor.',
  );
  assert.equal(await selected(page), 'anonymous');
  console.log(
    'PASS delayed start cancellation and fresh typed full-page fallback',
  );
  await page.close();

  const expired = await newPage(EXPIRED);
  await expired.goto(origin);
  await expect(
    expired.getByRole('button', { name: 'Continue as Carbon', exact: true }),
  ).toBeEnabled();
  assert.equal(await selected(expired), 'anonymous');
  await expired
    .getByRole('button', { name: 'Continue as Carbon', exact: true })
    .click();
  await expect.poll(() => starts.length).toBe(3);
  const fresh = starts[2].attempt_nonce;
  assert.equal(
    requests.findLast(
      (item) => item.page === expired && item.path === '/browser/login/start',
    ).selector,
    'anonymous',
  );
  await expired
    .getByRole('button', { name: 'Cancel sign-in', exact: true })
    .click();
  await expect.poll(() => cancellations.includes(fresh)).toBe(true);
  held.get(fresh).resolve(409);
  await expired.close();
  console.log(
    'PASS an expired selected context is cleared before a fresh anonymous sign-in',
  );

  const left = await newPage(A),
    right = await newPage(B);
  await Promise.all([left.goto(origin), right.goto(origin)]);
  await expect(
    left.getByRole('textbox', { name: 'Search filenames and contents' }),
  ).toBeVisible();
  await expect(
    right.getByRole('textbox', { name: 'Search filenames and contents' }),
  ).toBeVisible();
  await context.addCookies([
    { name: 'fixture_shared_selection', value: B, url: origin },
  ]);
  await Promise.all([left.reload(), right.reload()]);
  await expect(
    left.getByRole('textbox', { name: 'Search filenames and contents' }),
  ).toBeVisible();
  await expect(
    right.getByRole('textbox', { name: 'Search filenames and contents' }),
  ).toBeVisible();
  assert(sessionRequests(left).every((item) => item.selector === A));
  assert(sessionRequests(right).every((item) => item.selector === B));
  assert.equal(await selected(left), A);
  assert.equal(await selected(right), B);
  const unknown = await newPage(UNKNOWN);
  await unknown.goto(origin);
  await expect(unknown.getByRole('alert')).toContainText(
    'Selected context is unavailable',
  );
  assert.equal(await selected(unknown), UNKNOWN);
  assert(sessionRequests(unknown).every((item) => item.selector === UNKNOWN));
  await unknown.close();
  console.log(
    'PASS two-tab selectors survive shared-cookie replacement/reload; unknown selector never falls back',
  );

  await left.evaluate(() =>
    sessionStorage.setItem('briefcase-login-kind', 'carbon'),
  );
  firstActivation = deferred();
  activationGate = deferred();
  const arrived = firstActivation.promise;
  await left.goto(`${origin}/?iam_activate=${'a'.repeat(64)}&context=${C}`);
  await arrived;
  await expect.poll(() => left.url()).toBe(`${origin}/`);
  assert.equal(
    await selected(left),
    A,
    'An unfinished candidate must not replace the selected account.',
  );
  await left.reload();
  await left.waitForURL(`${origin}/org/proof-c/`);
  await expect(
    left.getByRole('textbox', { name: 'Search filenames and contents' }),
  ).toBeVisible();
  activationGate.resolve();
  await right.reload();
  await expect(
    right.getByRole('textbox', { name: 'Search filenames and contents' }),
  ).toBeVisible();
  assert.equal(await selected(left), C);
  assert.equal(await selected(right), B);
  assert.equal(activations.length, 2);
  assert.deepEqual(activations[0], activations[1]);
  assert.equal(sessionRequests(left).at(-1).selector, C);
  assert(sessionRequests(right).every((item) => item.selector === B));
  assert.equal(
    await left.evaluate(() => sessionStorage.getItem('briefcase-login-return')),
    null,
  );
  console.log(
    'PASS full-page candidate activation retries the same receipt after unload; another tab stays selected',
  );
  assert.deepEqual(pageErrors, []);
} finally {
  for (const gate of held.values()) gate.resolve(409);
  activationGate?.resolve();
  await context.close().catch(() => {});
  await browser.close();
  await server.close();
}
