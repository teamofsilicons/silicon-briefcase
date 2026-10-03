import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  openIamPopup,
  completeIamPopup,
  PopupBlockedError,
  continueIamInThisTab,
  safeLoginReturn,
  beginLoginAttempt,
  bindLoginAttempt,
  savedLoginAttempt,
  retireLoginAttempt,
  retiredLoginAttempts,
} from '../lib/iam-popup.ts';

function browser(t) {
  const oldWindow = globalThis.window,
    oldHistory = globalThis.history;
  const listeners = new Map();
  const popup = {
    closed: false,
    location: { href: '' },
    close() {
      this.closed = true;
    },
  };
  const window = {
    location: { origin: 'https://app.example', href: 'https://app.example/' },
    open: () => popup,
    addEventListener: (name, fn) => listeners.set(name, fn),
    removeEventListener: (name) => listeners.delete(name),
    opener: null,
    close() {},
  };
  globalThis.window = window;
  globalThis.history = { replaceState() {} };
  t.after(() => {
    globalThis.window = oldWindow;
    globalThis.history = oldHistory;
  });
  return { window, popup, send: (data) => listeners.get('message')?.(data) };
}
void test('popup completion requires the exact origin, opened window and unpredictable nonce', async (t) => {
  const b = browser(t);
  let nonce,
    completed = false;
  const result = openIamPopup((value) => {
    nonce = value;
    return '/start?nonce=' + value;
  }).then(() => {
    completed = true;
  });
  await Promise.resolve();
  assert.match(nonce, /^[a-f0-9]{64}$/);
  const good = {
    origin: 'https://app.example',
    source: b.popup,
    data: {
      type: 'silicon:iam-login-complete',
      nonce,
      result: 'ok',
      context: '11111111-1111-4111-8111-111111111111',
    },
  };
  b.send({ ...good, origin: 'https://wrong.example' });
  b.send({ ...good, source: {} });
  b.send({ ...good, data: { ...good.data, nonce: '0'.repeat(64) } });
  await Promise.resolve();
  assert.equal(completed, false);
  b.send({ ...good, data: { ...good.data, context: undefined } });
  assert.equal(completed, false);
  b.send(good);
  await result;
  assert.equal(completed, true);
  assert.equal(b.popup.closed, true);
});
void test('blocked popups fail clearly and completion never sends callback credentials', async (t) => {
  const b = browser(t);
  b.window.open = () => null;
  await assert.rejects(
    openIamPopup(() => '/start'),
    PopupBlockedError,
  );
  let message, origin;
  b.window.opener = {
    postMessage(value, target) {
      message = value;
      origin = target;
    },
  };
  b.window.location.href =
    'https://app.example/?iam_popup=complete&nonce=' +
    'a'.repeat(64) +
    '&result=ok&context=11111111-1111-4111-8111-111111111111&slt=must-not-be-forwarded';
  assert.equal(completeIamPopup(), true);
  assert.equal(origin, 'https://app.example');
  assert.deepEqual(message, {
    type: 'silicon:iam-login-complete',
    nonce: 'a'.repeat(64),
    result: 'ok',
    context: '11111111-1111-4111-8111-111111111111',
  });
});

void test('aborting a popup closes it and ignores its late reply', async (t) => {
  const b = browser(t);
  const controller = new AbortController();
  const pending = openIamPopup(() => '/start', controller.signal);
  controller.abort();
  await assert.rejects(pending, /cancelled/);
  assert.equal(b.popup.closed, true);
  b.send({ data: { result: 'ok' } });
  await assert.rejects(
    openIamPopup(() => '/start', controller.signal),
    /cancelled/,
  );
});

void test('blocked popup permits an explicit full-page start and cancelled starts never navigate', async (t) => {
  const b = browser(t);
  let assigned,
    starts = 0;
  b.window.open = () => null;
  b.window.location.assign = (url) => {
    assigned = url;
  };
  await assert.rejects(
    openIamPopup(() => {
      starts++;
      return '/start';
    }),
    PopupBlockedError,
  );
  assert.equal(starts, 0);
  b.window.open = () => {
    throw new Error('popups disabled');
  };
  await assert.rejects(
    openIamPopup(() => '/start'),
    PopupBlockedError,
  );
  assert.equal(assigned, undefined);
  await continueIamInThisTab(() => {
    starts++;
    return 'https://iam.example/login?identity_kind=silicon';
  });
  assert.equal(starts, 1);
  assert.equal(assigned, 'https://iam.example/login?identity_kind=silicon');
  const controller = new AbortController();
  assigned = undefined;
  let resolve;
  const pending = continueIamInThisTab(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
    controller.signal,
  );
  controller.abort();
  resolve('/must-not-navigate');
  await assert.rejects(pending, /cancelled/);
  assert.equal(assigned, undefined);
});

void test('full-page completion permits only canonical local workspace paths', (t) => {
  browser(t);
  assert.equal(safeLoginReturn('/'), '/');
  assert.equal(safeLoginReturn('/org/tos/files'), '/org/tos/files');
  for (const value of [
    'https://evil.example',
    '//evil.example',
    '/org/../evil',
    '/org/tos?redirect=evil',
    '/org/tos#bad',
    undefined,
  ])
    assert.throws(() => safeLoginReturn(value), /Invalid/);
});

void test('a newer same-kind attempt retires the persisted full-page predecessor', (t) => {
  browser(t);
  const previous = globalThis.sessionStorage;
  const values = new Map();
  globalThis.sessionStorage = {
    getItem: (k) => values.get(k) ?? null,
    setItem: (k, v) => values.set(k, v),
    removeItem: (k) => values.delete(k),
  };
  t.after(() => {
    globalThis.sessionStorage = previous;
  });
  const a = 'a'.repeat(64),
    b = 'b'.repeat(64),
    stateA = 'c'.repeat(64),
    stateB = 'd'.repeat(64);
  const redirect = (state) =>
    'https://iam.example/login?redirect_uri=' +
    encodeURIComponent('https://app.example/auth/callback?state=' + state);
  beginLoginAttempt(a, 'carbon');
  bindLoginAttempt(a, redirect(stateA));
  assert.equal(savedLoginAttempt().state, stateA);
  beginLoginAttempt(b, 'carbon');
  bindLoginAttempt(b, redirect(stateB));
  assert.deepEqual(retiredLoginAttempts(), [a]);
  assert.equal(savedLoginAttempt().state, stateB);
  assert.throws(() => bindLoginAttempt(a, redirect(stateA)), /superseded/);
  retireLoginAttempt(a);
  assert.equal(savedLoginAttempt().nonce, b);
});
