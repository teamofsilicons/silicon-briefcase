"use client";
import { useEffect, useSyncExternalStore } from "react";
import { Switch } from "@/components/silicon-ui/switch/switch";
import { setTelemetryEnabled, startTelemetry, telemetryEnabled } from "@/lib/telemetry";
function subscribe(update: () => void) { window.addEventListener("briefcase-telemetry-change",update);window.addEventListener("storage",update);return()=>{window.removeEventListener("briefcase-telemetry-change",update);window.removeEventListener("storage",update);}; }
export function TelemetryProvider() {useEffect(startTelemetry,[]);return null;}
export function TelemetryPreference() {const enabled=useSyncExternalStore(subscribe,telemetryEnabled,()=>true);return <Switch label="Share usage and diagnostic events" checked={enabled} onCheckedChange={setTelemetryEnabled} />;}
