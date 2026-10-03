import { telemetryEnabled, trackRequest } from './telemetry';
export type BrowserSession = {
  authenticated: boolean;
  context_id: string;
  contexts: {
    context_id: string;
    org: string;
    actor: { type: string; public_id: string };
  }[];
  org: string;
  organizations: string[];
  actor: { type: 'carbon' | 'silicon'; public_id: string };
  testing: boolean;
  test_environment?: { id: string; name: string } | null;
};
export type AccountSession = Omit<BrowserSession, 'org'> & { org: null };
export type Entry = {
  id: string;
  name: string;
  path: string;
  type: 'file' | 'folder';
  root_type: string;
  tag?: string;
  size: number | null;
  updated_at: string | null;
  content_type: string | null;
  render: string | null;
  effective_access: string[];
  visibility: string;
  owner: { type: string; id: string } | null;
  permanent_url: string;
  deleted_at: string | null;
  /** When a self-destructing file is deleted for good. */
  self_destruct_at?: string | null;
};
export type Page = { items: Entry[]; next_cursor: string | null };
export class ApiError extends Error {
  constructor(
    message: string,
    public status: number,
  ) {
    super(message);
  }
}
// Per-tab request context. This is a consistency guard, not an authorization
// credential: the gateway and IAM still authorize every file request.
let workspaceOrganization: string | null = null;
let accountContext: string | null = null;
let contextEpoch = 0;
let selectorLoaded = false;
let anonymousSelected = false;
const selectorKey = () =>
  'briefcase-context:' + (testingEnvironment() || 'production');
function loadSelector() {
  if (selectorLoaded || typeof window === 'undefined') return;
  const saved = sessionStorage.getItem(selectorKey());
  if (
    saved &&
    saved !== 'anonymous' &&
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
      saved,
    )
  )
    throw new ApiError(
      'The saved account selection is invalid. Sign in again.',
      409,
    );
  accountContext = saved && saved !== 'anonymous' ? saved : null;
  anonymousSelected = saved === 'anonymous';
  selectorLoaded = true;
}
export const browserContextGeneration = () => {
  loadSelector();
  return contextEpoch;
};
export function setAccountContext(context: string | null) {
  sessionStorage.setItem(selectorKey(), context || 'anonymous');
  selectorLoaded = true;
  anonymousSelected = context === null;
  if (accountContext !== context) contextEpoch += 1;
  accountContext = context;
}
export function setWorkspaceOrganization(org: string | null) {
  workspaceOrganization = org;
}
export function testingEnvironment(): string | null {
  return typeof window === 'undefined'
    ? null
    : sessionStorage.getItem('briefcase-test-environment');
}
export function enterTestingEnvironment(id: string, context: string) {
  if (
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
      context,
    )
  )
    throw new ApiError('The testing account could not be verified.', 409);
  sessionStorage.setItem('briefcase-context:' + id, context);
  contextEpoch += 1;
  sessionStorage.setItem('briefcase-test-environment', id);
  selectorLoaded = false;
  loadSelector();
  window.location.assign('/');
}
export function returnToProduction() {
  contextEpoch += 1;
  sessionStorage.removeItem('briefcase-test-environment');
  selectorLoaded = false;
  loadSelector();
  window.location.assign('/');
}
export function browserUrl(path: string): string {
  loadSelector();
  const params = new URLSearchParams();
  const id = testingEnvironment();
  if (id) params.set('test_environment', id);
  if (accountContext) params.set('account_context', accountContext);
  else if (anonymousSelected) params.set('account_context', 'anonymous');
  return params.size ? path + (path.includes('?') ? '&' : '?') + params : path;
}
export async function api<T>(
  path: string,
  method = 'GET',
  body?: unknown,
): Promise<T> {
  loadSelector();
  const context = accountContext;
  const epoch = contextEpoch;
  const environment = testingEnvironment();
  const options: RequestInit = {
    method,
    credentials: 'same-origin',
    headers: {
      'Content-Type': 'application/json',
      'X-Briefcase-Browser': '1',
      ...(context
        ? { 'X-Briefcase-Context': context }
        : anonymousSelected
          ? { 'X-Briefcase-Context': 'anonymous' }
          : {}),
      'X-Briefcase-Telemetry': telemetryEnabled() ? 'on' : 'off',
      ...(workspaceOrganization && path !== '/session'
        ? { 'X-Briefcase-Organization': workspaceOrganization }
        : {}),
    },
    redirect: 'error',
  };
  if (body !== undefined) {
    if (method === 'GET' || method === 'HEAD')
      throw new Error('A read request cannot have a body.');
    options.body = JSON.stringify(body);
  }
  const started = performance.now();
  const response = await fetch(browserUrl('/browser' + path), options);
  trackRequest(method, response.status, performance.now() - started);
  if (
    contextEpoch !== epoch ||
    accountContext !== context ||
    testingEnvironment() !== environment
  )
    throw new ApiError(
      'The account changed while this request was running.',
      409,
    );
  const value = (await response.json().catch(() => null)) as {
    error?: { message?: string };
  } | null;
  if (
    contextEpoch !== epoch ||
    accountContext !== context ||
    testingEnvironment() !== environment
  )
    throw new ApiError(
      'The account changed while this response was arriving.',
      409,
    );
  if (!response.ok)
    throw new ApiError(
      value?.error?.message || 'Briefcase could not complete this request.',
      response.status,
    );
  if (value === null)
    throw new ApiError('Briefcase returned an unreadable response.', 502);
  return value as T;
}
export function bytes(value: number | null | undefined) {
  if (value == null) return '—';
  if (value === 0) return '0 B';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  const n = Math.min(Math.floor(Math.log(value) / Math.log(1024)), 4);
  return (
    (value / 1024 ** n).toLocaleString(undefined, {
      maximumFractionDigits: n ? 1 : 0,
    }) +
    ' ' +
    units[n]
  );
}
export function date(value: string | null) {
  return value
    ? new Date(value).toLocaleDateString(undefined, {
        day: 'numeric',
        month: 'short',
        year: 'numeric',
      })
    : '—';
}
