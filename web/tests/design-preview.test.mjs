import test from 'node:test';
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { request as httpRequest } from 'node:http';
import { createPreviewServer } from '../scripts/design-preview.mjs';

async function preview(t, options) {
  const server = createPreviewServer(options);
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(() => new Promise((resolve) => server.close(resolve)));
  const origin = `http://127.0.0.1:${server.address().port}`;
  const request = (path, method = 'GET', body, headers = {}) => {
    const options = {
      method,
      headers: { 'Content-Type': 'application/json', ...headers },
    };
    if (body !== undefined) options.body = JSON.stringify(body);
    return fetch(origin + '/browser' + path, options);
  };
  return { request, origin };
}

void test('preview rejects nonlocal origins and hosts before changing state', async (t) => {
  const { request, origin } = await preview(t);
  assert.equal(
    (
      await request('/session', 'DELETE', undefined, {
        Origin: 'https://example.com',
      })
    ).status,
    403,
  );
  const hostStatus = await new Promise((resolve, reject) => {
    const req = httpRequest(
      origin + '/browser/entries',
      { headers: { Host: 'example.com' } },
      (response) => {
        response.resume();
        resolve(response.statusCode);
      },
    );
    req.on('error', reject);
    req.end();
  });
  assert.equal(hostStatus, 403);
  assert.equal(
    (
      await request('/entries', 'GET', undefined, {
        'X-Briefcase-Organization': 'production',
      })
    ).status,
    403,
  );
  assert.equal((await (await request('/session')).json()).authenticated, true);
  assert.equal(
    (await request('/storage/configuration', 'PUT', {})).status,
    501,
  );
});

void test('session simulation is explicitly local and restart-isolated', async (t) => {
  const { request } = await preview(t, { signedOut: true });
  const session = await (await request('/session')).json();
  assert.equal(session.authenticated, false);
  assert.equal(session.org, 'design-preview');
  assert.equal(session.actor.public_id, 'c:preview');
  assert.equal((await request('/entries')).status, 401);
  const login = await (
    await request('/login/start', 'POST', { return_to: 'https://example.com' })
  ).json();
  assert.equal(login.redirect_url, '/');
  assert.equal((await request('/entries')).status, 200);
  await request('/session', 'DELETE');
  assert.equal((await request('/entries')).status, 401);
  const second = await preview(t);
  assert.equal((await second.request('/entries')).status, 200);
});

void test('folder changes preserve descendant paths, prevent cycles, and restore bin state', async (t) => {
  const { request } = await preview(t);
  const campaign = await (
    await request('/entries?path=Design%2FAutumn%20campaign')
  ).json();
  assert.equal(campaign.items.length, 6);
  assert.deepEqual(
    Object.fromEntries(
      campaign.items
        .filter((entry) => entry.type === 'file')
        .map((entry) => [entry.name, entry.render]),
    ),
    {
      'Brand guide.pdf': 'document',
      'Campaign cover.png': 'image',
      'Launch film.mp4': 'video',
      'Website copy.md': 'document',
    },
  );
  assert.equal(
    campaign.items.find((entry) => entry.name === 'Campaign cover.png')
      .content_type,
    'image/png',
  );
  assert.equal(
    (await request('/entries', 'POST', { name: '../escape', parent: 'Design' }))
      .status,
    400,
  );
  const body = {
    name: 'Concepts',
    parent: 'Design',
    operation_id: 'create-once',
  };
  const folder = await (await request('/entries', 'POST', body)).json();
  assert.equal(
    (await (await request('/entries', 'POST', body)).json()).id,
    folder.id,
  );
  assert.equal(
    (await request('/entries/campaign', 'PATCH', { parent_id: 'assets' }))
      .status,
    400,
  );
  const moved = await (
    await request('/entries/campaign', 'PATCH', { parent_id: folder.id })
  ).json();
  assert.equal(moved.path, 'Design/Concepts/Autumn campaign');
  assert.equal(
    (await (await request('/entries/brand-guide')).json()).path,
    'Design/Concepts/Autumn campaign/Brand guide.pdf',
  );
  await request('/entries/campaign', 'DELETE');
  assert.equal((await request('/entries/brand-guide')).status, 404);
  await request('/bin/campaign/restore', 'POST', {});
  assert.equal((await request('/entries/brand-guide')).status, 200);
  const oldDraft = await (await request('/bin')).json();
  assert.deepEqual(
    oldDraft.items.map((entry) => entry.id),
    ['old-copy'],
  );
});

void test('previews contain valid file bytes and public access only opens after enabling a link', async (t) => {
  const { request, origin } = await preview(t);
  const pdf = await request('/entries/brand-guide/content', 'GET', undefined, {
    Range: 'bytes=0-7',
  });
  assert.equal(pdf.status, 206);
  assert.equal(await pdf.text(), '%PDF-1.4');
  assert.match(pdf.headers.get('content-range'), /^bytes 0-7\/\d+$/);
  assert.match(
    await (await request('/entries/website-copy/content')).text(),
    /^# A little more space/,
  );
  const cover = await request('/entries/campaign-cover/content');
  assert.equal(cover.headers.get('content-type'), 'image/png');
  const imageBytes = Buffer.from(await cover.arrayBuffer());
  assert.deepEqual(
    imageBytes.subarray(0, 8),
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
  );
  const publicPath =
    '/public?org=design-preview&path=Design%2FAutumn%20campaign%2FWebsite%20copy.md';
  assert.equal((await request(publicPath)).status, 404);
  await request('/entries/campaign/link-access', 'PUT', { enabled: true });
  assert.equal((await request(publicPath)).status, 200);
  const uploaded = await fetch(
    origin +
      '/browser/upload?parent=Design&name=Notes.txt&content_type=text%2Fplain',
    { method: 'POST', body: 'A local note.' },
  );
  const entry = await uploaded.json();
  assert.equal(entry.size, 13);
  assert.equal(
    await (await request(`/entries/${entry.id}/content`)).text(),
    'A local note.',
  );
  const download = await request('/entries/brand-guide/content?download=true');
  assert.match(download.headers.get('content-disposition'), /^attachment;/);
});

void test('controlled upload rejection and lost response keep retry behavior testable', async (t) => {
  const { request, origin } = await preview(t);
  const upload = (name, operation) => {
    const params = new URLSearchParams({
      parent: 'Design',
      name,
      operation_id: operation,
      content_type: 'text/plain',
    });
    return fetch(`${origin}/browser/upload?${params}`, {
      method: 'POST',
      body: 'Retry fixture.',
    });
  };
  assert.equal((await upload('rejected.txt', 'reject-once')).status, 413);
  await assert.rejects(
    upload('recover-on-retry.txt', 'recover-once'),
    /fetch failed/,
  );
  const recovered = await (
    await upload('recover-on-retry.txt', 'recover-once')
  ).json();
  assert.equal(recovered.name, 'recover-on-retry.txt');
  assert.equal(
    await (await request(`/entries/${recovered.id}/content`)).text(),
    'Retry fixture.',
  );
  const listing = await (await request('/entries?path=Design')).json();
  assert.equal(
    listing.items.filter((entry) => entry.name === 'recover-on-retry.txt')
      .length,
    1,
  );
  assert.equal(
    listing.items.some((entry) => entry.name === 'rejected.txt'),
    false,
  );
});

void test('preview selects only its public saved context, never an organization substitute', async (t) => {
  const { request } = await preview(t);
  const session = await (await request('/session')).json();
  assert.equal(session.contexts.length, 1);
  assert.equal(session.contexts[0].context_id, session.context_id);
  assert.equal(
    (await request('/session', 'PATCH', { org: session.org })).status,
    403,
  );
  assert.equal(
    (await request('/session', 'PATCH', { context_id: session.context_id }))
      .status,
    200,
  );
});
