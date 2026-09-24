'use client';
import { useEffect, useRef, useState } from 'react';
import { Hourglass, Timer } from 'lucide-react';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';

// Expiring shares and self destruct both last 1 minute to 30 days, in whole minutes.
export const MAX_LIFETIME_MINUTES = 43_200;
const UNITS = { minutes: 1, hours: 60, days: 1440 } as const;
type Unit = keyof typeof UNITS;
export type Duration = { amount: string; unit: Unit };
export const DEFAULT_DURATION: Duration = { amount: '1', unit: 'days' };
const PRESETS: { label: string; value: Duration }[] = [
  { label: '10 min', value: { amount: '10', unit: 'minutes' } },
  { label: '1 hour', value: { amount: '1', unit: 'hours' } },
  { label: '1 day', value: { amount: '1', unit: 'days' } },
  { label: '7 days', value: { amount: '7', unit: 'days' } },
  { label: '30 days', value: { amount: '30', unit: 'days' } },
];

/** Whole minutes for a valid duration, otherwise null. */
export function durationMinutes(duration: Duration): number | null {
  const amount = duration.amount.trim();
  if (!/^\d+$/.test(amount)) return null;
  const minutes = Number(amount) * UNITS[duration.unit];
  return minutes >= 1 && minutes <= MAX_LIFETIME_MINUTES ? minutes : null;
}

export function spokenDuration(minutes: number): string {
  const [amount, unit] =
    minutes % 1440 === 0
      ? [minutes / 1440, 'day']
      : minutes % 60 === 0
        ? [minutes / 60, 'hour']
        : [minutes, 'minute'];
  return amount + ' ' + unit + (amount === 1 ? '' : 's');
}

export function absoluteTime(at: string): string {
  return new Date(at).toLocaleString(undefined, {
    day: 'numeric',
    month: 'short',
    year: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function timeLeft(at: string, now: number): string {
  const minutes = Math.ceil((Date.parse(at) - now) / 60_000);
  if (minutes <= 0) return 'ending now';
  if (minutes < 60) return minutes + ' min left';
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    const rest = minutes % 60;
    return hours + ' h' + (rest ? ' ' + rest + ' min' : '') + ' left';
  }
  const days = Math.floor(hours / 24),
    rest = hours % 24;
  return (
    days +
    (days === 1 ? ' day' : ' days') +
    (rest ? ' ' + rest + ' h' : '') +
    ' left'
  );
}

function useNow(interval = 10_000) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), interval);
    return () => window.clearInterval(timer);
  }, [interval]);
  return now;
}

/**
 * Time left on an expiring share or a self-destructing file. Calls `onElapsed`
 * once when the time passes, so the caller can drop or refresh what ended.
 */
export function Lifetime({
  at,
  kind,
  onElapsed,
}: {
  at: string;
  kind: 'expiring' | 'self-destruct';
  onElapsed?: () => void;
}) {
  const now = useNow();
  const elapsed = Date.parse(at) <= now;
  const reported = useRef(false);
  useEffect(() => {
    if (elapsed && !reported.current) {
      reported.current = true;
      onElapsed?.();
    }
  }, [elapsed, onElapsed]);
  const Icon = kind === 'expiring' ? Timer : Hourglass;
  return (
    <time
      className="lifetime-badge"
      data-kind={kind}
      dateTime={at}
      title={
        (kind === 'expiring' ? 'Access ends ' : 'Deleted for good ') +
        absoluteTime(at)
      }
    >
      <Icon size={13} aria-hidden="true" />
      {kind === 'expiring' ? 'Expires' : 'Self-destructs'} · {timeLeft(at, now)}
    </time>
  );
}

export function DurationPicker({
  id,
  value,
  onChange,
  disabled,
}: {
  id: string;
  value: Duration;
  onChange: (value: Duration) => void;
  disabled?: boolean;
}) {
  const valid = durationMinutes(value) !== null;
  return (
    <div className="duration-picker">
      <div className="duration-fields">
        <Input
          id={id}
          type="number"
          inputMode="numeric"
          min={1}
          max={MAX_LIFETIME_MINUTES / UNITS[value.unit]}
          step={1}
          required
          disabled={disabled}
          aria-invalid={!valid}
          value={value.amount}
          onChange={(e) => onChange({ ...value, amount: e.target.value })}
        />
        <Select
          value={value.unit}
          disabled={disabled}
          onValueChange={(unit) => {
            if (unit === 'minutes' || unit === 'hours' || unit === 'days')
              onChange({ ...value, unit });
          }}
        >
          <SelectTrigger aria-label="Unit">
            <SelectValue>
              {value.unit === 'minutes'
                ? 'Minutes'
                : value.unit === 'hours'
                  ? 'Hours'
                  : 'Days'}
            </SelectValue>
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="minutes">Minutes</SelectItem>
            <SelectItem value="hours">Hours</SelectItem>
            <SelectItem value="days">Days</SelectItem>
          </SelectContent>
        </Select>
      </div>
      <div className="duration-presets">
        {PRESETS.map((preset) => (
          <button
            key={preset.label}
            type="button"
            disabled={disabled}
            aria-pressed={
              preset.value.amount === value.amount.trim() &&
              preset.value.unit === value.unit
            }
            onClick={() => onChange(preset.value)}
          >
            {preset.label}
          </button>
        ))}
      </div>
      {!valid && (
        <p className="field-note" role="alert">
          Choose 1 minute to 30 days, in whole minutes.
        </p>
      )}
    </div>
  );
}
