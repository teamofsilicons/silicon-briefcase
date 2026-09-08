export type BrowserSession = {
  authenticated: boolean;
  org: string;
  organizations: string[];
  actor: { type: string; public_id: string };
  testing: boolean;
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
export async function api<T>(
  path: string,
  method = 'GET',
  body?: unknown,
): Promise<T> {
  const options: RequestInit = {
    method,
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json', 'X-Briefcase-Browser': '1' },
    redirect: 'error',
  };
  if (body !== undefined) {
    if (method === 'GET' || method === 'HEAD')
      throw new Error('A read request cannot have a body.');
    options.body = JSON.stringify(body);
  }
  const response = await fetch('/browser' + path, options);
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
