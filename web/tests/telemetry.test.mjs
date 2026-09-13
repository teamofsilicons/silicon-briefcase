import test from 'node:test';
import assert from 'node:assert/strict';
import { safeBrowserEvent, startTelemetry, setTelemetryEnabled, trackRequest } from '../lib/telemetry.ts';

const pause = () => new Promise(resolve => setTimeout(resolve, 1150));
const storage = () => { const values = new Map(); return { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key) }; };

test('sanitizer discards all content and keeps bounded diagnostics', () => {
  const environment = crypto.randomUUID();
  const raw = { id: crypto.randomUUID(), type: 'network', data: { url: 'https://host/private/secret.txt?slt_private', status: 503, duration_ms: 42, message: 'private text', percent: 120 }, metadata: { token: 'slt_private' } };
  const event = safeBrowserEvent(raw, environment);
  assert.equal(event.id, raw.id);
  assert.equal(event.environment_id, environment);
  assert.equal(event.testing, true);
  assert.equal(event.stage, 'failed');
  assert.equal(event.duration_ms, 42);
  assert.equal(event.progress, null);
  assert.doesNotMatch(JSON.stringify(event), /secret|private|https|token/);
  assert.equal(safeBrowserEvent({type: 'slt_private', data: {duration_ms: Infinity}}, null).operation, 'web_unknown');
});

test('official browser analytics and UI events use one sanitized relay; opt-out clears queued work', async () => {
  const calls = [];
  globalThis.localStorage = storage();
  globalThis.sessionStorage = storage();
  globalThis.location = { protocol: 'https:', origin: 'https://briefcase.test', pathname: '/private/secret.txt', href: 'https://briefcase.test/private/secret.txt?token=slt_private' };
  const document = Object.assign(new EventTarget(), {cookie: '', referrer: 'https://private.example/slt_private', documentElement: {scrollHeight:1000}, readyState:'complete'});
  globalThis.document = document;
  const window = Object.assign(new EventTarget(), { document, history: {}, innerHeight: 500, scrollY: 0, fetch: async (url, options) => { calls.push({url, options, body: JSON.parse(options.body)}); return new Response(null, {status:204}); } });
  globalThis.window = window;
  const environment = crypto.randomUUID();
  sessionStorage.setItem('briefcase-test-environment', environment);
  const stop = startTelemetry();
  try {
    trackRequest('POST', 201, 12);
    // A queued event retains its original plane even during navigation away.
    sessionStorage.removeItem('briefcase-test-environment');
    await pause();
    assert.ok(calls.length);
    const events = calls.flatMap(call => call.body.events);
    assert.ok(events.some(event => event.operation === 'web_page_view'));
    assert.ok(events.some(event => event.operation === 'web_action_post'));
    assert.ok(events.every(event => event.environment_id === environment));
    for (const call of calls) {
      assert.equal(call.url, '/browser/telemetry');
      assert.equal(call.body.table, 'siliconbriefcase');
      assert.equal(call.options.headers['x-briefcase-browser'], '1');
      assert.doesNotMatch(JSON.stringify(call), /secret|slt_private|private\.example/);
    }
    const before = calls.length;
    trackRequest('PUT', 500, 20);
    setTelemetryEnabled(false);
    await pause();
    assert.equal(calls.length, before);
    assert.match(document.cookie, /briefcase_telemetry=off/);
    setTelemetryEnabled(true);
    trackRequest('GET', 200, 7);
    await pause();
    assert.ok(calls.length > before);
  } finally { stop(); }
});
