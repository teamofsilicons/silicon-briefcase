"use client";

/**
 * The sidebar's width in steps, as on Silicon Accounts (its components/foundation/shell/sidebar-resize.tsx): a handle on
 * the page box's left edge, whose grip forms on hover, snaps the sidebar between SIDEBAR_STEPS as it is dragged. Arrow
 * keys step, Home and End jump to the limits, a double-click goes back to the default. The width is remembered in this
 * browser.
 */
import { useRef, useState, useSyncExternalStore, type KeyboardEvent, type PointerEvent } from "react";
import styles from "./shell.module.css";

export const SIDEBAR_STEPS = [200, 248, 296] as const;
const DEFAULT_STEP = 1;
const KEY = "briefcase.sidebar-step";

function readStep(): number {
  try {
    const stored = window.localStorage.getItem(KEY);
    const step = stored === null ? DEFAULT_STEP : Number(stored);
    return Number.isInteger(step) && step >= 0 && step < SIDEBAR_STEPS.length ? step : DEFAULT_STEP;
  } catch {
    return DEFAULT_STEP;
  }
}

// The step lives in this browser's storage; the shell reads it as an external store (the default on the server).
const listeners = new Set<() => void>();
const subscribe = (change: () => void) => {
  listeners.add(change);
  return () => listeners.delete(change);
};
const serverStep = () => DEFAULT_STEP;

function writeStep(next: number) {
  const clamped = Math.min(SIDEBAR_STEPS.length - 1, Math.max(0, next));
  try {
    window.localStorage.setItem(KEY, String(clamped));
  } catch {
    // Storage off: nothing to remember the width in, so it stays where it is.
  }
  listeners.forEach(change => change());
}

/** The chosen step (an index into SIDEBAR_STEPS) and its setter, which remembers it. */
export function useSidebarStep(): [number, (step: number) => void] {
  return [useSyncExternalStore(subscribe, readStep, serverStep), writeStep];
}

/** The step whose width is nearest `width`. */
function nearestStep(width: number): number {
  let best = 0;
  SIDEBAR_STEPS.forEach((candidate, index) => {
    if (Math.abs(candidate - width) < Math.abs(SIDEBAR_STEPS[best] - width)) best = index;
  });
  return best;
}

export function SidebarResize({ step, onStep, controls }: { step: number; onStep: (step: number) => void; controls: string }) {
  const [dragging, setDragging] = useState(false);
  /** Where the sidebar starts, in client px, for the drag in progress. */
  const origin = useRef(0);

  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    const aside = document.getElementById(controls);
    if (!aside) return;
    event.preventDefault();
    origin.current = aside.getBoundingClientRect().left;
    event.currentTarget.setPointerCapture(event.pointerId);
    setDragging(true);
  };
  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    if (!dragging) return;
    const next = nearestStep(event.clientX - origin.current);
    if (next !== step) onStep(next);
  };
  const endDrag = (event: PointerEvent<HTMLDivElement>) => {
    if (!dragging) return;
    event.currentTarget.releasePointerCapture(event.pointerId);
    setDragging(false);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const next = { ArrowLeft: step - 1, ArrowRight: step + 1, Home: 0, End: SIDEBAR_STEPS.length - 1 }[event.key];
    if (next === undefined) return;
    event.preventDefault();
    onStep(next);
  };

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Sidebar width"
      aria-controls={controls}
      aria-valuemin={SIDEBAR_STEPS[0]}
      aria-valuemax={SIDEBAR_STEPS[SIDEBAR_STEPS.length - 1]}
      aria-valuenow={SIDEBAR_STEPS[step]}
      aria-valuetext={`${SIDEBAR_STEPS[step]} pixels`}
      tabIndex={0}
      className={styles.resize}
      data-active={dragging ? "" : undefined}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onKeyDown={onKeyDown}
      onDoubleClick={() => onStep(DEFAULT_STEP)}
    >
      <span className={styles.resizeGrip} aria-hidden="true" />
    </div>
  );
}
