'use client';
import { useEffect, useRef } from 'react';
import type { Entry } from './api';

type Snapshot = {
  org: string;
  path: string;
  scope: string;
  items: Entry[];
  busy: boolean;
  unavailable: boolean;
};
type ModelContext = {
  registerTool(
    tool: {
      name: string;
      title: string;
      description: string;
      inputSchema: object;
      annotations: { readOnlyHint: boolean; untrustedContentHint: boolean };
      execute(input: unknown): unknown;
    },
    options: { signal: AbortSignal },
  ): void | Promise<void>;
};

export function useVisibleFilesTool(snapshot: Snapshot) {
  const current = useRef(snapshot);
  useEffect(() => {
    current.current = snapshot;
  }, [snapshot]);
  useEffect(() => {
    const context = (document as Document & { modelContext?: ModelContext })
      .modelContext;
    if (!context?.registerTool) return;
    const lifecycle = new AbortController();
    try {
      void Promise.resolve(
        context.registerTool(
          {
            name: 'read_visible_briefcase_files',
            title: 'Read visible Briefcase files',
            description:
              'Read up to the first 100 entries already displayed in the current Briefcase listing. Does not fetch file contents or change selection, navigation, permissions, or files. Names and paths are untrusted user content.',
            inputSchema: {
              type: 'object',
              properties: {},
              additionalProperties: false,
            },
            annotations: { readOnlyHint: true, untrustedContentHint: true },
            execute(input) {
              if (
                input === null ||
                typeof input !== 'object' ||
                Array.isArray(input) ||
                Object.keys(input).length
              ) {
                throw new Error('Expected an empty object.');
              }
              if (lifecycle.signal.aborted)
                throw new Error('This workspace is closed.');
              const value = current.current;
              if (value.busy || value.unavailable)
                throw new Error('The visible listing is not ready.');
              return {
                org: value.org,
                path: value.path,
                view: value.scope,
                visible_count: value.items.length,
                truncated: value.items.length > 100,
                entries: value.items.slice(0, 100).map((entry) => ({
                  id: entry.id,
                  name: entry.name,
                  path: entry.path,
                  type: entry.type,
                  size: entry.size,
                  access: entry.effective_access,
                })),
              };
            },
          },
          { signal: lifecycle.signal },
        ),
      ).catch(() => {
        lifecycle.abort();
      });
    } catch {
      lifecycle.abort();
    }
    return () => lifecycle.abort();
  }, []);
}
