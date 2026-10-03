import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import ts from 'typescript';
const source = await readFile(new URL('../lib/api.ts', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } }).outputText.replace("'./telemetry'", JSON.stringify(new URL('../lib/telemetry.ts', import.meta.url).href));
const api = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`);
const values = new Map();
globalThis.window = {};
globalThis.sessionStorage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key) };
globalThis.localStorage = { getItem: () => 'off' };

test('requests and media retain the account and testing context that initiated them', async () => {
  api.setAccountContext('account-a');
  api.setWorkspaceOrganization('org-a');
  values.set('briefcase-test-environment', 'test-a');
  let finish;
  let request;
  globalThis.fetch = (url, options) => {
    request = { url, options };
    return new Promise(resolve => { finish = resolve; });
  };
  const pending = api.api('/entries', 'POST', { name: 'draft' });
  assert.equal(request.options.headers['X-Briefcase-Context'], 'account-a');
  assert.equal(request.options.headers['X-Briefcase-Organization'], 'org-a');
  assert.match(request.url, /test_environment=test-a/);
  assert.match(request.url, /account_context=account-a/);
  api.setAccountContext('account-b');
  finish(new Response(JSON.stringify({ id: 'created-in-a' })));
  await assert.rejects(pending, error => error.status === 409);
  assert.match(api.browserUrl('/browser/entries/file/content'), /account_context=account-b/);
  values.clear();
});

test('same-context responses are accepted and switching never replays a mutation', async () => {
  api.setAccountContext('account-c');
  let calls = 0;
  globalThis.fetch = async () => { calls += 1; return new Response(JSON.stringify({ id: 'saved' })); };
  assert.deepEqual(await api.api('/entries', 'POST', { name: 'draft' }), { id: 'saved' });
  assert.equal(calls, 1);
});

test('a delayed JSON body is fenced even after switching away and back', async () => {
  api.setAccountContext('account-body');
  let finishBody;
  let startedBody;
  const bodyStarted = new Promise(resolve => { startedBody = resolve; });
  globalThis.fetch = async () => ({
    ok: true, status: 200,
    json() { startedBody(); return new Promise(resolve => { finishBody = resolve; }); },
  });
  const pending = api.api('/entries');
  await bodyStarted;
  api.setAccountContext('different-account');
  api.setAccountContext('account-body');
  finishBody({ items: [{ id: 'old-result' }] });
  await assert.rejects(pending, error => error.status === 409);
});

test('test re-entry replaces only its world selector, preserving production after an anonymous visit', () => {
  const production = '11111111-1111-4111-8111-111111111111';
  const world = '22222222-2222-4222-8222-222222222222';
  const next = '33333333-3333-4333-8333-333333333333';
  values.clear();
  api.setAccountContext(production);
  values.set('briefcase-context:' + world, 'anonymous');
  globalThis.window.location = { assign() {} };
  api.enterTestingEnvironment(world, next);
  assert.equal(values.get('briefcase-context:' + world), next);
  assert.equal(values.get('briefcase-context:production'), production);
  assert.match(api.browserUrl('/browser/session'), new RegExp('account_context=' + next));
  api.returnToProduction();
  assert.match(api.browserUrl('/browser/session'), new RegExp('account_context=' + production));
  assert.throws(() => api.enterTestingEnvironment(world, '------------------------------------'), /verified/);
});

test('anonymous selection is explicit and survives a late cookie without adopting that account', async () => {
  api.setAccountContext(null);
  globalThis.fetch = async (url, options) => {
    assert.equal(options.headers['X-Briefcase-Context'], 'anonymous');
    assert.match(url, /account_context=anonymous/);
    return new Response(JSON.stringify({ authenticated: false }));
  };
  assert.deepEqual(await api.api('/session'), { authenticated: false });
  assert.equal(values.get('briefcase-context:production'), 'anonymous');
});
