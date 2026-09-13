import {
  createSpaceStationWeb,
  type SpaceStationWeb,
} from '@teamofsilicons/space-station-web';

let collector: SpaceStationWeb | undefined;
const preference = 'briefcase-telemetry';
export function telemetryEnabled(): boolean {
  if (typeof window === 'undefined') return true;
  try {
    return localStorage.getItem(preference) !== 'off';
  } catch {
    return false;
  }
}
export function setTelemetryEnabled(enabled: boolean) {
  try {
    localStorage.setItem(preference, enabled ? 'on' : 'off');
  } catch {
    enabled = false;
  }
  document.cookie = `briefcase_telemetry=${enabled ? 'on' : 'off'}; Path=/; SameSite=Strict; Max-Age=31536000${location.protocol === 'https:' ? '; Secure' : ''}`;
  collector?.setEnabled(enabled);
  window.dispatchEvent(new Event('briefcase-telemetry-change'));
}

const allowed = new Set([
  'page_view',
  'page_exit',
  'click',
  'error',
  'scroll',
  'timing',
  'network',
  'network_error',
  'action_get',
  'action_post',
  'action_put',
  'action_patch',
  'action_delete',
]);
const uuid = (value: unknown): string | null =>
  typeof value === 'string' &&
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
    value,
  ) &&
  value !== '00000000-0000-0000-0000-000000000000'
    ? value
    : null;
const number = (
  value: unknown,
  max = Number.MAX_SAFE_INTEGER,
): number | null =>
  typeof value === 'number' &&
  Number.isFinite(value) &&
  value >= 0 &&
  value <= max
    ? Math.round(value)
    : null;

// Space Station's package collects browser context. Reduce it before it leaves
// the browser: filenames, URLs, referrers, error messages and input values are
// never transmitted to either Briefcase or Space Station.
export function safeBrowserEvent(
  raw: { id?: unknown; type?: unknown; data?: Record<string, unknown> },
  environment: string | null,
) {
  const type =
    typeof raw.type === 'string' && allowed.has(raw.type)
      ? raw.type
      : 'unknown';
  const data = raw.data || {};
  const status = number(data.status, 599);
  return {
    id: uuid(raw.id) || crypto.randomUUID(),
    source: 'web',
    operation: `web_${type}`,
    stage:
      type === 'error' ||
      type === 'network_error' ||
      (status !== null && status >= 400)
        ? 'failed'
        : 'completed',
    testing: !!environment,
    environment_id: uuid(environment),
    request_id: null,
    duration_ms: number(data.duration_ms ?? data.elapsed_ms ?? data.load_ms),
    status: status !== null && status >= 100 ? status : null,
    attempt: null,
    count: null,
    progress: number(data.percent, 100),
  };
}

export function startTelemetry() {
  const transport = window.fetch.bind(window);
  // Plane changes reload the document; queued events retain their original plane.
  const environment = sessionStorage.getItem('briefcase-test-environment');
  setTelemetryEnabled(telemetryEnabled());
  collector = createSpaceStationWeb({
    analyticsTable: 'siliconbriefcase',
    eventsTable: 'siliconbriefcase',
    endpoint: '/browser/telemetry',
    enabled: telemetryEnabled(),
    fetch: async (url, init) => {
      if (!telemetryEnabled()) return new Response(null, { status: 204 });
      if (typeof init?.body !== 'string') return new Response(null, { status: 400 });
      const body = JSON.parse(init.body) as {
        events: Parameters<typeof safeBrowserEvent>[0][];
      };
      return transport(url, {
        ...init,
        credentials: 'same-origin',
        headers: {
          'content-type': 'application/json',
          'x-briefcase-browser': '1',
        },
        body: JSON.stringify({
          table: 'siliconbriefcase',
          events: body.events.map((event) =>
            safeBrowserEvent(event, environment),
          ),
        }),
      });
    },
  });
  const changed = () => collector?.setEnabled(telemetryEnabled());
  window.addEventListener('storage', changed);
  return () => {
    window.removeEventListener('storage', changed);
    const current = collector;
    collector = undefined;
    void current?.destroy();
  };
}

export function trackRequest(method: string, status: number, duration: number) {
  const operation = `action_${method.toLowerCase()}`;
  if (allowed.has(operation))
    collector?.track(operation, { status, duration_ms: duration });
}
