export type BrowserSession = {
  authenticated: boolean;
  org: string;
  organizations: string[];
  actor: { type: string; public_id: string };
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
export function setWorkspaceOrganization(org: string | null) {
  workspaceOrganization = org;
}
export function testingEnvironment(): string | null {
  return typeof window === 'undefined'
    ? null
    : sessionStorage.getItem('briefcase-test-environment');
}
export function enterTestingEnvironment(id: string) {
  sessionStorage.setItem('briefcase-test-environment', id);
  window.location.assign('/');
}
export function returnToProduction() {
  sessionStorage.removeItem('briefcase-test-environment');
  window.location.assign('/');
}
export function browserUrl(path: string): string {
  const id = testingEnvironment();
  return id
    ? path +
        (path.includes('?') ? '&' : '?') +
        'test_environment=' +
        encodeURIComponent(id)
    : path;
}
export async function api<T>(
  path: string,
  method = 'GET',
  body?: unknown,
): Promise<T> {
  const options: RequestInit = {
    method,
    credentials: 'same-origin',
    headers: {
      'Content-Type': 'application/json',
      'X-Briefcase-Browser': '1',
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
  const response = await fetch(browserUrl('/browser' + path), options);
  const value = (await response.json().catch(() => null)) as {
    error?: { message?: string };
  } | null;
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
