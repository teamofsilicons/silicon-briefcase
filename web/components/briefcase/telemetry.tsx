'use client';
import { useEffect, useSyncExternalStore } from 'react';
import {
  startTelemetry,
  telemetryEnabled,
  setTelemetryEnabled,
} from '@/lib/telemetry';

export function TelemetryProvider() {
  useEffect(startTelemetry, []);
  return null;
}

export function TelemetryPreference() {
  const enabled = useSyncExternalStore(subscribe, telemetryEnabled, () => true);
  return (
    <label className="telemetry-preference">
      <input
        type="checkbox"
        checked={enabled}
        onChange={(event) => {
          setTelemetryEnabled(event.target.checked);
        }}
      />{' '}
      Share usage and diagnostic events{' '}
      <span>
        Applies to this browser. File contents, names, credentials and typed
        text are excluded.
      </span>
    </label>
  );
}

function subscribe(update: () => void) {
  window.addEventListener('briefcase-telemetry-change', update);
  window.addEventListener('storage', update);
  return () => {
    window.removeEventListener('briefcase-telemetry-change', update);
    window.removeEventListener('storage', update);
  };
}
