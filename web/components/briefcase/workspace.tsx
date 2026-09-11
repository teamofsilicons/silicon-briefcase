'use client';
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type SubmitEvent,
} from 'react';
import {
  Folder,
  File as FileIcon,
  Globe,
  LockKeyhole,
  Tags,
  Trash2,
  Clock3,
  Search,
  Plus,
  Upload,
  ArrowUpRight,
  Download,
  MoreHorizontal,
  LogOut,
  RefreshCw,
  Link as LinkIcon,
  X,
} from 'lucide-react';
import {
  SidebarProvider,
  Sidebar,
  SidebarContent,
  SidebarHeader,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuItem,
  SidebarMenuButton,
  SidebarInset,
  SidebarTrigger,
} from '@/components/ui/sidebar';
import {
  Table,
  TableHeader,
  TableRow,
  TableHead,
  TableBody,
  TableCell,
} from '@/components/ui/table';
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
  SheetDescription,
} from '@/components/ui/sheet';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog';
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogCancel,
} from '@/components/ui/alert-dialog';
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs';
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from '@/components/ui/dropdown-menu';
import { Checkbox } from '@/components/ui/checkbox';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  api,
  ApiError,
  bytes,
  date,
  type Entry,
  type Page,
  type BrowserSession,
} from '@/lib/api';

import { fileLocation, readFileLocation } from '@/lib/file-location';
import { textPreview } from '@/lib/text-preview';
import { textFormat } from '@/lib/preview-document';
import { richPreview } from '@/lib/rich-preview';
import Notifications from './notifications';
import OrganizationSettings from './organization-settings';
import TestingEnvironments from './testing-environments';
import { useVisibleFilesTool } from '@/lib/use-visible-files-tool';

type Scope = 'files' | 'recent' | 'bin' | 'search';
type Editor = {
  kind: 'folder' | 'rename' | 'move' | 'share';
  entry?: Entry;
  value: string;
  operation: string;
  parent?: string;
  rootType?: 'public' | 'private' | 'tag';
  tag?: string;
};
type Usage = {
  storage: { used_bytes: number; limit_bytes: number; remaining_bytes: number };
  daily_uploads: { used_bytes: number; limit_bytes: number };
};
type Version = {
  id: string;
  number: number;
  size: number;
  created_at: string;
  created_by: { id: string };
};
type Activity = {
  action: string;
  actor: { type: string; id: string };
  app_id: string | null;
  occurred_at: string;
};
type Grant = {
  id: string;
  principal: { type: string; id: string };
  access: string[];
  inherit: boolean;
};

function invalidateRequestGeneration(counter: { current: number }) {
  // Invalidate the latest asynchronous lookup. This is not a rendered DOM ref.
  counter.current += 1;
}

export default function Workspace({
  session,
  onSignOut,
  onChooseOrganization,
}: {
  session: BrowserSession;
  onSignOut: () => void;
  onChooseOrganization: () => void;
}) {
  const [scope, setScope] = useState<Scope>('files'),
    [path, setPath] = useState(''),
    [items, setItems] = useState<Entry[]>([]),
    [roots, setRoots] = useState<Entry[]>([]),
    [cursor, setCursor] = useState<string | null>(null),
    [loading, setLoading] = useState(true),
    [error, setError] = useState(''),
    [notice, setNotice] = useState('');
  const [query, setQuery] = useState(''),
    [search, setSearch] = useState(''),
    [filter, setFilter] = useState(''),
    [advanced, setAdvanced] = useState(false),
    [usage, setUsage] = useState<Usage | null>(null),
    [selected, setSelected] = useState<Entry | null>(null),
    [tab, setTab] = useState('preview');
  const [editor, setEditor] = useState<Editor | null>(null),
    [confirm, setConfirm] = useState<Entry | null>(null),
    [working, setWorking] = useState(false),
    [rights, setRights] = useState<string[]>(['read']);
  const [versions, setVersions] = useState<Version[]>([]),
    [grants, setGrants] = useState<Grant[]>([]),
    [activity, setActivity] = useState<Activity[]>([]),
    [detailError, setDetailError] = useState(''),
    [detailLoading, setDetailLoading] = useState(false);
  const [uploadProgress, setUploadProgress] = useState<number | null>(null),
    [retryUpload, setRetryUpload] = useState(false);
  const [routeLoading, setRouteLoading] = useState(true),
    [missingPath, setMissingPath] = useState<string | null>(null),
    [locationError, setLocationError] = useState(''),
    [parent, setParent] = useState<Entry | null>(null);
  const [preview, setPreview] = useState<{
    id: string;
    document: string;
    note: string;
    truncated: boolean;
  } | null>(null);
  const [navigation, setNavigation] = useState(0);
  useVisibleFilesTool({
    org: session.org,
    path,
    scope,
    items,
    busy: loading || routeLoading,
    unavailable: !!error || !!locationError || !!missingPath,
  });
  const routeGeneration = useRef(0),
    versionIntents = useRef(new Map<string, string>()),
    binRestoreIntents = useRef(new Map<string, string>());
  const fileInput = useRef<HTMLInputElement>(null),
    uploadIntent = useRef<{
      file: File;
      parent: string;
      operation: string;
    } | null>(null),
    generation = useRef(0);
  const fail = useCallback(
    (e: unknown) => {
      if (e instanceof ApiError && e.status === 401) {
        onSignOut();
        return;
      }
      setError(
        e instanceof Error ? e.message : 'The request could not be completed.',
      );
    },
    [onSignOut],
  );
  const resolveLocation = useCallback(async () => {
    const ticket = ++routeGeneration.current;
    generation.current++;
    setRouteLoading(true);
    setItems([]);
    setCursor(null);
    setParent(null);
    setSelected(null);
    setMissingPath(null);
    setLocationError('');
    setScope('files');
    setFilter('');
    let target: ReturnType<typeof readFileLocation> = null;
    try {
      target = readFileLocation();
      if (target && target.org !== session.org) {
        setLocationError(
          'This link belongs to another organisation. Sign out and sign in to ' +
            target.org +
            ' to open it.',
        );
        return;
      }
      if (!target?.path) {
        setPath('');
        return;
      }
      const entry = await api<Entry>(
        '/resolve?path=' + encodeURIComponent(target.path),
      );
      if (ticket !== routeGeneration.current) return;
      if (entry.type === 'folder') setPath(entry.path);
      else {
        setPath(entry.path.split('/').slice(0, -1).join('/'));
        setSelected(entry);
        setTab('preview');
      }
    } catch (e) {
      if (ticket !== routeGeneration.current) return;
      if (e instanceof ApiError && e.status === 404 && target?.path) {
        setMissingPath(target.path);
        setPath('');
      } else {
        setLocationError(e instanceof Error ? e.message : 'File not found.');
        if (e instanceof ApiError && e.status === 401) onSignOut();
      }
    } finally {
      if (ticket === routeGeneration.current) {
        setRouteLoading(false);
        setLoading(false);
        setNavigation((value) => value + 1);
      }
    }
  }, [session.org, onSignOut]);
  useEffect(() => {
    // eslint-disable-next-line react/react-compiler -- Synchronize browser navigation with a cancellable SDK lookup; stable dependencies prevent a render loop.
    void resolveLocation();
    const back = () => {
      void resolveLocation();
    };
    window.addEventListener('popstate', back);
    return () => {
      invalidateRequestGeneration(routeGeneration);
      window.removeEventListener('popstate', back);
    };
  }, [resolveLocation]);
  const load = useCallback(
    async (next?: string) => {
      if (routeLoading || missingPath || locationError) return;
      const ticket = ++generation.current;
      setLoading(true);
      setError('');
      try {
        let data: Page;
        if (scope === 'search') {
          const result = await api<{ entry: Entry }[]>(
            '/search?q=' + encodeURIComponent(search),
          );
          data = { items: result.map((r) => r.entry), next_cursor: null };
        } else if (scope === 'bin')
          data = await api<Page>(
            '/bin' + (next ? '?cursor=' + encodeURIComponent(next) : ''),
          );
        else {
          const p = new URLSearchParams();
          if (path) p.set('path', path);
          if (scope === 'recent') p.set('filter', 'last:20 sort:newest');
          else if (filter) p.set('filter', filter);
          if (next) p.set('cursor', next);
          const [page, folder] = await Promise.all([
            api<Page>('/entries?' + p),
            path
              ? api<Entry>('/resolve?path=' + encodeURIComponent(path))
              : Promise.resolve(null),
          ]);
          data = page;
          if (ticket === generation.current) setParent(folder);
        }
        if (ticket === generation.current) {
          const listedItems = data.items;
          setItems((previous) =>
            next ? [...previous, ...listedItems] : listedItems,
          );
          setCursor(data.next_cursor);
        }
      } catch (e) {
        if (ticket === generation.current) fail(e);
      } finally {
        if (ticket === generation.current) setLoading(false);
      }
    },
    [
      scope,
      path,
      search,
      filter,
      fail,
      routeLoading,
      missingPath,
      locationError,
    ],
  );
  useEffect(() => {
    // eslint-disable-next-line react/react-compiler -- Set pending state when starting an external listing request; response writes are generation-fenced.
    void load();
  }, [load, navigation]);
  useEffect(() => {
    api<Page>('/entries')
      .then((p) => setRoots(p.items))
      .catch(fail);
    api<Usage>('/usage').then(setUsage).catch(fail);
  }, [fail]);
  function navigate(p: string, s: Scope = 'files') {
    setNavigation((value) => value + 1);
    routeGeneration.current++;
    generation.current++;
    setRouteLoading(false);
    setMissingPath(null);
    setLocationError('');
    setItems([]);
    setCursor(null);
    setParent(null);
    setPath(p);
    setScope(s);
    setSelected(null);
    setFilter('');
    setNotice('');
    history.pushState(null, '', fileLocation(session.org, p));
  }
  function closeDetails() {
    setSelected(null);
    history.replaceState(null, '', fileLocation(session.org, path));
  }
  async function refreshed(message: string) {
    setNotice(message);
    await load();
    api<Usage>('/usage').then(setUsage).catch(fail);
  }
  function open(entry: Entry) {
    if (entry.type === 'folder' && scope !== 'bin') {
      navigate(entry.path);
      return;
    }
    setSelected(entry);
    setTab('preview');
    if (scope !== 'bin')
      history.pushState(null, '', fileLocation(session.org, entry.path));
  }
  function edit(kind: Editor['kind'], entry?: Entry) {
    setRights(['read']);
    setEditor({
      kind,
      entry,
      value: kind === 'rename' ? entry?.name || '' : '',
      operation: crypto.randomUUID(),
      parent: kind === 'folder' ? path : undefined,
      rootType: kind === 'folder' && !path ? 'private' : undefined,
    });
    setError('');
  }
  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!editor) return;
    setWorking(true);
    setError('');
    try {
      let created: Entry | undefined;
      if (editor.kind === 'folder')
        created = await api<Entry>('/entries', 'POST', {
          name: editor.value,
          parent: editor.parent,
          root_type: editor.rootType,
          tag: editor.rootType === 'tag' ? editor.tag : undefined,
          operation_id: editor.operation,
        });
      if (editor.kind === 'rename')
        await api('/entries/' + editor.entry!.id, 'PATCH', {
          name: editor.value,
          operation_id: editor.operation,
        });
      if (editor.kind === 'move') {
        const target = await api<Entry>(
          '/resolve?path=' + encodeURIComponent(editor.value),
        );
        if (target.type !== 'folder')
          throw new Error('Choose a destination folder.');
        await api('/entries/' + editor.entry!.id, 'PATCH', {
          parent_id: target.id,
          operation_id: editor.operation,
        });
      }
      if (editor.kind === 'share') {
        const [type, ...name] = editor.value.split(':');
        if (!['carbon', 'silicon'].includes(type) || !name.join(':'))
          throw new Error('Use carbon:member-id or silicon:member-id.');
        await api('/entries/' + editor.entry!.id + '/permissions', 'POST', {
          principal: { type, id: name.join(':') },
          access: rights,
          inherit: editor.entry!.type === 'folder',
        });
      }
      setEditor(null);
      setSelected(null);
      if (created && editor.parent === '') {
        navigate(created.path.split('/').slice(0, -1).join('/'));
        setNotice('Created ' + created.name + '.');
        return;
      }
      await refreshed(editor.kind === 'share' ? 'Access updated.' : 'Saved.');
    } catch (e) {
      fail(e);
    } finally {
      setWorking(false);
    }
  }
  async function remove() {
    if (!confirm) return;
    setWorking(true);
    try {
      await api('/entries/' + confirm.id, 'DELETE');
      setConfirm(null);
      setSelected(null);
      await refreshed('Moved to the bin. Recoverable for 45 days.');
    } catch (e) {
      fail(e);
    } finally {
      setWorking(false);
    }
  }
  async function restore(entry: Entry) {
    if (working) return;
    // Retain the identity even after success: a failed listing refresh can
    // leave the old Bin row visible. A later deletion gets a distinct identity.
    const cycle = entry.id + ':' + entry.deleted_at;
    let operation = binRestoreIntents.current.get(cycle);
    if (!operation) {
      operation = crypto.randomUUID();
      binRestoreIntents.current.set(cycle, operation);
    }
    setWorking(true);
    try {
      await api('/bin/' + entry.id + '/restore', 'POST', {
        operation_id: operation,
      });
      setSelected(null);
      await refreshed('Restored from the bin.');
    } catch (e) {
      fail(e);
    } finally {
      setWorking(false);
    }
  }
  async function upload(file?: File) {
    if (file)
      uploadIntent.current = {
        file,
        parent: path,
        operation: crypto.randomUUID(),
      };
    const intent = uploadIntent.current;
    if (!intent) return;
    setUploadProgress(0);
    setRetryUpload(false);
    setError('');
    const p = new URLSearchParams({
      parent: intent.parent,
      name: intent.file.name,
      content_type: intent.file.type || 'application/octet-stream',
      operation_id: intent.operation,
    });
    const xhr = new XMLHttpRequest();
    xhr.open('POST', '/browser/upload?' + p);
    xhr.setRequestHeader('X-Briefcase-Browser', '1');
    xhr.setRequestHeader('X-Briefcase-Organization', session.org);
    xhr.timeout = 1800000;
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable)
        setUploadProgress(Math.round((e.loaded / e.total) * 100));
    };
    const failed = (message: string) => {
      setUploadProgress(null);
      setRetryUpload(true);
      setError(message);
    };
    xhr.onerror = () =>
      failed(
        'The upload response was lost. Retry this same upload to recover its result.',
      );
    xhr.ontimeout = () =>
      failed(
        'The upload timed out. Retry this same upload to recover its result.',
      );
    xhr.onload = () => {
      let result;
      try {
        result = JSON.parse(xhr.responseText);
      } catch {
        failed('Unable to read the upload result. Retry the same upload.');
        return;
      }
      if (xhr.status < 200 || xhr.status >= 300) {
        failed(result.error?.message || 'Upload failed.');
        return;
      }
      setUploadProgress(null);
      uploadIntent.current = null;
      void refreshed('Uploaded ' + intent.file.name + '.');
    };
    xhr.send(intent.file);
  }
  useEffect(() => {
    let current = true;
    const abort = new AbortController();
    // eslint-disable-next-line react/react-compiler -- Clear the previous entry's preview before starting a cancellable request for the new selection.
    setDetailError('');
    setPreview(null);
    setVersions([]);
    setGrants([]);
    setActivity([]);
    if (!selected || scope === 'bin') {
      setDetailLoading(false);
      return;
    }
    setDetailLoading(true);
    const run = async () => {
      try {
        if (tab === 'versions') {
          const v = await api<Version[]>(
            '/entries/' + selected.id + '/versions',
          );
          if (current) setVersions(v);
        } else if (tab === 'access') {
          const g = await api<Grant[]>(
            '/entries/' + selected.id + '/permissions',
          );
          if (current) setGrants(g);
        } else if (tab === 'activity') {
          const a = await api<Activity[]>(
            '/entries/' + selected.id + '/activity',
          );
          if (current) setActivity(a);
        } else if (selected.type === 'file') {
          const format = textFormat(selected);
          if (format) {
            const excerpt = await textPreview(
              selected.id,
              abort.signal,
              selected.size === 0,
            );
            const rendered = await richPreview(
              excerpt.text,
              format,
              abort.signal,
            );
            if (current)
              setPreview({
                id: selected.id,
                ...rendered,
                truncated: excerpt.truncated,
              });
          }
        }
      } catch (e) {
        if (current)
          setDetailError(
            e instanceof Error ? e.message : 'Unable to load details.',
          );
      } finally {
        if (current) setDetailLoading(false);
      }
    };
    void run();
    return () => {
      current = false;
      abort.abort();
    };
  }, [selected, tab, scope]);
  const title =
    scope === 'bin'
      ? 'Bin'
      : scope === 'recent'
        ? 'Recent files'
        : scope === 'search'
          ? 'Search results'
          : path
            ? path.split('/').at(-1)
            : 'All files';
  const canUpload =
    scope === 'files' &&
    !routeLoading &&
    !loading &&
    !missingPath &&
    !locationError &&
    !!path &&
    path !== 'private' &&
    !!parent?.effective_access.includes('write');
  const contentUrl = selected
    ? '/browser/entries/' + selected.id + '/content'
    : '';
  const canCreateFolder =
    canUpload ||
    (scope === 'files' &&
      !path &&
      !routeLoading &&
      !loading &&
      !missingPath &&
      !locationError);
  // Multiple top-level folders may share one IAM tag boundary.
  const writableTags = [
    ...new Map(
      roots
        .filter(
          (root) =>
            root.root_type === 'tag' &&
            root.tag &&
            root.effective_access.includes('write'),
        )
        .map((root) => [root.tag, root]),
    ).values(),
  ];
  return (
    <SidebarProvider>
      <Sidebar className="briefcase-sidebar">
        <SidebarHeader>
          {/* Full-page navigation intentionally resets the workspace and its browser-bound state. */}
          {/* eslint-disable-next-line next/no-html-link-for-pages */}
          <a href="/" className="brand workspace-brand">
            {/* eslint-disable-next-line next/no-img-element -- Local shared Silicon brand asset. */}
            <img src="/brand/mark.svg" alt="" width={28} height={28} />
            <strong>silicon</strong>
            <span>BRIEFCASE</span>
          </a>
          <div className="org-label">
            <div className="org-heading">ORGANISATION</div>
            <span>{session.org}</span>
            <Button variant="ghost" onClick={onChooseOrganization}>
              Workspaces & access
            </Button>
            <small>
              {session.testing
                ? 'Testing environment'
                : 'Organisation workspace'}
            </small>
          </div>
        </SidebarHeader>
        <SidebarContent>
          <SidebarGroup>
            <SidebarGroupLabel>LIBRARY</SidebarGroupLabel>
            <SidebarMenu>
              {[
                { label: 'All files', icon: Folder, p: '', s: 'files' },
                { label: 'Recent', icon: Clock3, p: '', s: 'recent' },
                { label: 'Public', icon: Globe, p: 'public', s: 'files' },
                {
                  label: 'My files',
                  icon: LockKeyhole,
                  p: 'private/' + session.actor.public_id,
                  s: 'files',
                },
                { label: 'Bin', icon: Trash2, p: '', s: 'bin' },
              ].map((n) => (
                <SidebarMenuItem key={n.label}>
                  <SidebarMenuButton
                    isActive={scope === n.s && path === n.p}
                    onClick={() => navigate(n.p, n.s as Scope)}
                  >
                    <n.icon />
                    <span>{n.label}</span>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              ))}
            </SidebarMenu>
          </SidebarGroup>
          {roots.some((r) => r.root_type === 'tag') && (
            <SidebarGroup>
              <SidebarGroupLabel>TEAM SPACES</SidebarGroupLabel>
              <SidebarMenu>
                {roots
                  .filter((r) => r.root_type === 'tag')
                  .map((r) => (
                    <SidebarMenuItem key={r.id}>
                      <SidebarMenuButton
                        onClick={() => navigate(r.path)}
                        isActive={path === r.path}
                      >
                        <Tags />
                        <span>{r.name}</span>
                      </SidebarMenuButton>
                    </SidebarMenuItem>
                  ))}
              </SidebarMenu>
            </SidebarGroup>
          )}
        </SidebarContent>
        <SidebarFooter>
          <TestingEnvironments session={session} onUnauthorized={onSignOut} />
          <OrganizationSettings session={session} onUnauthorized={onSignOut} />
          {usage && (
            <div className="storage-meter">
              <div>
                <span>Storage</span>
                <span>{bytes(usage.storage.used_bytes)}</span>
              </div>
              <meter
                min={0}
                max={usage.storage.limit_bytes || 1}
                value={usage.storage.used_bytes}
              />
              <small>
                {bytes(usage.storage.limit_bytes)} available capacity
              </small>
            </div>
          )}
          <div className="member">
            <span className="avatar">
              {session.actor.public_id.slice(0, 2).toUpperCase()}
            </span>
            <span>
              <strong>{session.actor.public_id}</strong>
              <small>{session.actor.type}</small>
            </span>
            <Button
              variant="ghost"
              size="icon"
              title="Sign out"
              aria-label="Sign out"
              onClick={async () => {
                try {
                  await api('/session', 'DELETE');
                  onSignOut();
                } catch (e) {
                  fail(e);
                }
              }}
            >
              <LogOut size={17} />
            </Button>
          </div>
        </SidebarFooter>
      </Sidebar>
      <SidebarInset className="workspace-main">
        <header className="workspace-top">
          <SidebarTrigger />
          <div className="workspace-context">
            <span>Silicon / Briefcase</span>
            <span className="workspace-environment">
              <i aria-hidden="true" />
              {session.testing ? 'Testing' : 'Production'}
            </span>
          </div>
          <form
            className="search-box"
            onSubmit={(e) => {
              e.preventDefault();
              if (query.trim()) {
                setSearch(query.trim());
                setScope('search');
                setPath('');
              }
            }}
          >
            <Search size={18} />
            <Input
              aria-label="Search filenames and contents"
              placeholder="Search your files and their contents"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            <Button variant="ghost" type="submit">
              Search
            </Button>
          </form>
          <Notifications
            onEntry={(entry) => {
              navigate(
                entry.type === 'folder'
                  ? entry.path
                  : entry.path.split('/').slice(0, -1).join('/'),
              );
              if (entry.type === 'file') {
                setSelected(entry);
                setTab('preview');
                history.replaceState(
                  null,
                  '',
                  fileLocation(session.org, entry.path),
                );
              }
            }}
            onUnauthorized={onSignOut}
          />
          <a
            className="docs-link"
            href="https://github.com/teamofsilicons/silicon-briefcase/tree/main/docs"
            target="_blank"
            rel="noreferrer"
          >
            Help <ArrowUpRight size={15} />
          </a>
        </header>
        <main className="file-surface">
          <nav className="breadcrumbs" aria-label="Folder path">
            <button onClick={() => navigate('')}>{session.org}</button>
            {path
              .split('/')
              .filter(Boolean)
              .map((part, i, all) => (
                <span key={i}>
                  {' '}
                  /{' '}
                  <button
                    onClick={() => navigate(all.slice(0, i + 1).join('/'))}
                  >
                    {part}
                  </button>
                </span>
              ))}
          </nav>
          <div className="file-title">
            <div>
              <div className="eyebrow">
                {scope === 'bin'
                  ? 'RECOVERABLE FOR 45 DAYS'
                  : session.testing
                    ? 'ISOLATED TEST DATA'
                    : 'YOUR LIBRARY'}
              </div>
              <h1>{title}</h1>
            </div>
            <div className="toolbar">
              {scope === 'files' && (
                <Button
                  variant="outline"
                  onClick={() => setAdvanced((v) => !v)}
                >
                  Filter
                </Button>
              )}
              <Button
                variant="outline"
                disabled={!canCreateFolder || working}
                onClick={() => edit('folder')}
              >
                <Plus size={16} /> New folder
              </Button>
              <Button
                disabled={!canUpload || uploadProgress !== null}
                onClick={() => fileInput.current?.click()}
              >
                <Upload size={16} /> Upload file
              </Button>
              <input
                ref={fileInput}
                type="file"
                hidden
                onChange={(e) => {
                  const file = e.target.files?.[0];
                  if (file) void upload(file);
                  e.currentTarget.value = '';
                }}
              />
            </div>
          </div>
          {advanced && scope === 'files' && (
            <form
              className="filter-bar"
              onSubmit={(e) => {
                e.preventDefault();
                const value = new FormData(e.currentTarget).get('filter');
                setFilter(typeof value === 'string' ? value : '');
              }}
            >
              <Input
                name="filter"
                aria-label="Advanced file filter"
                placeholder="is:pdf after:01-09-2026 sort:newest"
                defaultValue={filter}
              />
              <Button type="submit" variant="outline">
                Apply filter
              </Button>
            </form>
          )}
          {locationError && (
            <p className="error-box" role="alert">
              {locationError}
            </p>
          )}
          {missingPath && (
            <p className="error-box" role="alert">
              File not found
            </p>
          )}
          {parent &&
            (parent.visibility === 'traversal' ||
              (parent.root_type === 'private' &&
                parent.owner?.id !== session.actor.public_id)) && (
              <p className="detail-hint">
                You might not be seeing all the contents of this folder. This is
                a permission-based folder.
              </p>
            )}
          {error && (
            <div role="alert" className="error-box">
              {error}
              {retryUpload && (
                <Button variant="outline" onClick={() => void upload()}>
                  Retry same upload
                </Button>
              )}
            </div>
          )}
          {notice && (
            <output className="notice">
              {notice}
              <button aria-label="Dismiss" onClick={() => setNotice('')}>
                <X size={16} />
              </button>
            </output>
          )}
          {uploadProgress !== null && (
            <output className="upload-status">
              <Upload size={17} />
              {uploadProgress === 100
                ? 'Storing your file…'
                : `Uploading ${uploadProgress}%`}
              <progress value={uploadProgress} max={100} />
            </output>
          )}
          <Table className="file-table">
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead>Kind</TableHead>
                <TableHead>Modified</TableHead>
                <TableHead>Size</TableHead>
                <TableHead>
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {items.map((entry) => (
                <TableRow key={entry.id}>
                  <TableCell>
                    <button className="file-name" onClick={() => open(entry)}>
                      {entry.type === 'folder' ? (
                        <Folder size={22} />
                      ) : (
                        <FileIcon size={22} />
                      )}
                      <span>
                        <strong>{entry.name}</strong>
                        {scope !== 'files' && <small>{entry.path}</small>}
                        {entry.visibility === 'traversal' && (
                          <small>Only shared contents are visible</small>
                        )}
                      </span>
                    </button>
                  </TableCell>
                  <TableCell>
                    {entry.type === 'folder'
                      ? 'Folder'
                      : entry.render || 'File'}
                  </TableCell>
                  <TableCell>{date(entry.updated_at)}</TableCell>
                  <TableCell>
                    {entry.type === 'folder' ? '—' : bytes(entry.size)}
                  </TableCell>
                  <TableCell>
                    <DropdownMenu>
                      <DropdownMenuTrigger
                        render={
                          <Button
                            variant="ghost"
                            size="icon"
                            aria-label={'Actions for ' + entry.name}
                          />
                        }
                      >
                        <MoreHorizontal size={18} />
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        {scope === 'bin' ? (
                          <DropdownMenuItem onClick={() => void restore(entry)}>
                            Restore
                          </DropdownMenuItem>
                        ) : (
                          <>
                            <DropdownMenuItem
                              onClick={() => {
                                setSelected(entry);
                                setTab('activity');
                              }}
                            >
                              Details & history
                            </DropdownMenuItem>
                            {entry.type === 'file' && (
                              <DropdownMenuItem
                                onClick={() =>
                                  window.location.assign(
                                    '/browser/entries/' +
                                      entry.id +
                                      '/content?download=true',
                                  )
                                }
                              >
                                Download
                              </DropdownMenuItem>
                            )}
                            {entry.effective_access.includes('update') && (
                              <>
                                <DropdownMenuItem
                                  onClick={() => edit('rename', entry)}
                                >
                                  Rename
                                </DropdownMenuItem>
                                <DropdownMenuItem
                                  onClick={() => edit('move', entry)}
                                >
                                  Move
                                </DropdownMenuItem>
                              </>
                            )}
                            {entry.effective_access.includes(
                              'manage_permissions',
                            ) && (
                              <DropdownMenuItem
                                onClick={() => edit('share', entry)}
                              >
                                Share
                              </DropdownMenuItem>
                            )}
                            {entry.effective_access.includes('delete') && (
                              <DropdownMenuItem
                                variant="destructive"
                                onClick={() => setConfirm(entry)}
                              >
                                Move to bin
                              </DropdownMenuItem>
                            )}
                          </>
                        )}
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
          {(loading || routeLoading) && (
            <output className="empty-state">Loading files…</output>
          )}
          {!loading &&
            !routeLoading &&
            !missingPath &&
            !locationError &&
            !error &&
            items.length === 0 && (
              <div className="empty-state">
                <Folder size={38} />
                <h2>
                  {scope === 'search'
                    ? 'No matching files'
                    : scope === 'bin'
                      ? 'Your bin is empty'
                      : 'Nothing here yet'}
                </h2>
                <p>
                  {canUpload
                    ? 'Upload a file or create a folder to get started.'
                    : scope === 'search'
                      ? 'Try a different filename or a word inside a document.'
                      : 'Only files and folders you can access appear here.'}
                </p>
              </div>
            )}
          <div className="listing-footer">
            <span>
              {items.length} {items.length === 1 ? 'entry' : 'entries'}
            </span>
            {cursor && (
              <Button
                variant="outline"
                onClick={() => void load(cursor)}
                disabled={loading}
              >
                Load more
              </Button>
            )}
            <Button
              variant="ghost"
              onClick={() => void load()}
              disabled={loading}
            >
              <RefreshCw size={14} /> Refresh
            </Button>
          </div>
        </main>
      </SidebarInset>
      <Dialog
        open={!!editor}
        onOpenChange={(open) => {
          if (!open && !working) setEditor(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {editor?.kind === 'folder'
                ? 'New folder'
                : editor?.kind === 'share'
                  ? 'Share ' + editor.entry?.name
                  : editor?.kind === 'move'
                    ? 'Move ' + editor.entry?.name
                    : 'Rename ' + editor?.entry?.name}
            </DialogTitle>
            <DialogDescription>
              {editor?.kind === 'share'
                ? 'Grant access to an existing organisation member.'
                : 'Changes apply to this organisation and the selected environment.'}
            </DialogDescription>
          </DialogHeader>
          <form onSubmit={save}>
            {editor?.kind === 'folder' && editor.parent === '' && (
              <div className="folder-type-fields">
                <label htmlFor="folder-type">Folder type</label>
                <Select
                  value={editor.rootType}
                  disabled={working}
                  onValueChange={(value) => {
                    if (
                      value === 'public' ||
                      value === 'private' ||
                      value === 'tag'
                    )
                      setEditor((previous) =>
                        previous
                          ? {
                              ...previous,
                              rootType: value,
                              tag:
                                value === 'tag'
                                  ? writableTags[0]?.tag
                                  : undefined,
                            }
                          : null,
                      );
                  }}
                >
                  <SelectTrigger id="folder-type">
                    <SelectValue>
                      {editor.rootType === 'public'
                        ? 'Public'
                        : editor.rootType === 'tag'
                          ? 'Tag space'
                          : 'Private'}
                    </SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="private">Private</SelectItem>
                    <SelectItem value="public">Public</SelectItem>
                    <SelectItem value="tag" disabled={!writableTags.length}>
                      Tag space
                    </SelectItem>
                  </SelectContent>
                </Select>
                {editor.rootType === 'tag' && (
                  <>
                    <label htmlFor="folder-tag">Tag space</label>
                    <Select
                      value={editor.tag || null}
                      disabled={working}
                      onValueChange={(value) => {
                        if (typeof value === 'string')
                          setEditor((previous) =>
                            previous ? { ...previous, tag: value } : null,
                          );
                      }}
                    >
                      <SelectTrigger id="folder-tag">
                        <SelectValue>
                          {writableTags.find((root) => root.tag === editor.tag)
                            ?.tag || 'Choose a tag'}
                        </SelectValue>
                      </SelectTrigger>
                      <SelectContent>
                        {writableTags.map((root) => (
                          <SelectItem key={root.id} value={root.tag!}>
                            {root.tag}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </>
                )}
                <p className="detail-hint">
                  {editor.rootType === 'public'
                    ? 'Created at the top level. Everyone in this organisation can read its contents.'
                    : editor.rootType === 'tag'
                      ? 'Created at the top level. Members of the selected tag can read and add content.'
                      : 'Created at the top level. You control who can access it.'}
                </p>
              </div>
            )}
            <label htmlFor="editor-value">
              {editor?.kind === 'share'
                ? 'Member (carbon:id or silicon:id)'
                : editor?.kind === 'move'
                  ? 'Destination folder path'
                  : 'Name'}
            </label>
            <Input
              id="editor-value"
              disabled={working}
              required
              maxLength={255}
              value={editor?.value || ''}
              onChange={(e) =>
                setEditor((v) => (v ? { ...v, value: e.target.value } : null))
              }
            />
            {editor?.kind === 'share' && (
              <div className="rights">
                {['read', 'write', 'update', 'delete'].map((right) => (
                  <label key={right}>
                    <Checkbox
                      checked={rights.includes(right)}
                      disabled={working || right === 'read'}
                      onCheckedChange={(checked) =>
                        setRights((v) =>
                          checked
                            ? [...v, right]
                            : v.filter((r) => r !== right),
                        )
                      }
                    />
                    {right === 'write' ? 'Create new content' : right}
                  </label>
                ))}
              </div>
            )}
            {error && (
              <p className="error-box" role="alert">
                {error}
              </p>
            )}
            <Button
              type="submit"
              className="primary-action"
              disabled={working || (editor?.rootType === 'tag' && !editor.tag)}
            >
              {working ? 'Saving…' : 'Save'}
            </Button>
          </form>
        </DialogContent>
      </Dialog>
      <AlertDialog
        open={!!confirm}
        onOpenChange={(open) => {
          if (!open && !working) setConfirm(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              Move “{confirm?.name}” to the bin?
            </AlertDialogTitle>
            <AlertDialogDescription>
              It will disappear from your files. You can recover it for 45 days.
              A folder’s contents move with it.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {error && (
            <p role="alert" className="error-box">
              {error}
            </p>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={working}>Keep it</AlertDialogCancel>
            <Button
              variant="destructive"
              disabled={working}
              onClick={() => void remove()}
            >
              {working ? 'Moving…' : 'Move to bin'}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <Sheet
        open={!!selected}
        onOpenChange={(open) => {
          if (!open) closeDetails();
        }}
      >
        <SheetContent className="details-sheet sm:max-w-2xl">
          <SheetHeader>
            <SheetTitle>{selected?.name}</SheetTitle>
            <SheetDescription>{selected?.path}</SheetDescription>
          </SheetHeader>
          {selected && (
            <div className="detail-body">
              <div className="detail-actions">
                {selected.type === 'file' && scope !== 'bin' && (
                  <a
                    className="download-link"
                    href={contentUrl + '?download=true'}
                  >
                    <Download size={16} /> Download
                  </a>
                )}
                <Button
                  variant="outline"
                  onClick={async () => {
                    try {
                      await navigator.clipboard.writeText(
                        new URL(
                          fileLocation(session.org, selected.path),
                          location.origin,
                        ).href,
                      );
                      setNotice('File link copied.');
                    } catch (e) {
                      fail(e);
                    }
                  }}
                >
                  <LinkIcon size={15} /> Copy link
                </Button>
                {scope === 'bin' && (
                  <Button onClick={() => void restore(selected)}>
                    Restore
                  </Button>
                )}
              </div>
              <Tabs value={tab} onValueChange={setTab}>
                <TabsList>
                  <TabsTrigger value="preview">Preview</TabsTrigger>
                  <TabsTrigger value="access">Access</TabsTrigger>
                  {selected.type === 'file' && (
                    <TabsTrigger value="versions">Versions</TabsTrigger>
                  )}
                  <TabsTrigger value="activity">History</TabsTrigger>
                </TabsList>
                {detailError && <p className="error-box">{detailError}</p>}
                {detailLoading && <output>Loading…</output>}
                <TabsContent value="preview">
                  <div className="preview">
                    {scope === 'bin' ? (
                      <p>Restore this entry to open its content.</p>
                    ) : selected.type === 'folder' ? (
                      <Folder size={64} />
                    ) : selected.render === 'image' ? (
                      // Cookie-protected originals must not be fetched by a public image optimizer.
                      // eslint-disable-next-line next/no-img-element
                      <img src={contentUrl} alt={selected.name} />
                    ) : selected.render === 'video' ? (
                      // Originals may contain embedded captions; do not fabricate an external caption track.
                      // eslint-disable-next-line jsx-a11y/media-has-caption
                      <video
                        aria-label={selected.name}
                        controls
                        preload="metadata"
                        src={contentUrl}
                      />
                    ) : selected.render === 'audio' ? (
                      // eslint-disable-next-line jsx-a11y/media-has-caption -- Arbitrary uploaded audio does not include a supplied transcript.
                      <audio
                        aria-label={selected.name}
                        controls
                        preload="metadata"
                        src={contentUrl}
                      />
                    ) : preview?.id === selected.id ? (
                      <div className="text-preview">
                        {preview.truncated && (
                          <p className="detail-hint">
                            Showing the first 1 MiB. Download the file for its
                            complete contents.
                          </p>
                        )}
                        {preview.note && (
                          <p className="detail-hint">{preview.note}</p>
                        )}
                        <iframe
                          title={`${selected.name} preview`}
                          sandbox=""
                          referrerPolicy="no-referrer"
                          srcDoc={preview.document}
                        />
                      </div>
                    ) : selected.content_type === 'application/pdf' ? (
                      <iframe
                        title={selected.name}
                        sandbox=""
                        src={contentUrl}
                      />
                    ) : (
                      !detailLoading && (
                        <p>
                          No in-browser preview for this format. Download the
                          file to open it.
                        </p>
                      )
                    )}
                  </div>
                  <dl className="file-facts">
                    <div>
                      <dt>Size</dt>
                      <dd>{bytes(selected.size)}</dd>
                    </div>
                    <div>
                      <dt>Modified</dt>
                      <dd>{date(selected.updated_at)}</dd>
                    </div>
                    <div>
                      <dt>Owner</dt>
                      <dd>{selected.owner?.id || '—'}</dd>
                    </div>
                    <div>
                      <dt>Your access</dt>
                      <dd>{selected.effective_access.join(', ')}</dd>
                    </div>
                  </dl>
                </TabsContent>
                <TabsContent value="versions">
                  {versions.map((version) => (
                    <div className="detail-record" key={version.id}>
                      <div>
                        <strong>Version {version.number}</strong>
                        <p>
                          {bytes(version.size)} · {date(version.created_at)}
                        </p>
                      </div>
                      {selected.effective_access.includes('update') && (
                        <Button
                          variant="outline"
                          disabled={working}
                          onClick={async () => {
                            setWorking(true);
                            try {
                              const entry = await api<Entry>(
                                '/entries/' +
                                  selected.id +
                                  '/versions/' +
                                  version.id +
                                  '/restore',
                                'POST',
                                {
                                  operation_id: (() => {
                                    const key = selected.id + ':' + version.id;
                                    let intent =
                                      versionIntents.current.get(key);
                                    if (!intent) {
                                      intent = crypto.randomUUID();
                                      versionIntents.current.set(key, intent);
                                    }
                                    return intent;
                                  })(),
                                },
                              );
                              versionIntents.current.delete(
                                selected.id + ':' + version.id,
                              );
                              setSelected(entry);
                              await refreshed('Version restored.');
                            } catch (e) {
                              fail(e);
                            } finally {
                              setWorking(false);
                            }
                          }}
                        >
                          Restore
                        </Button>
                      )}
                    </div>
                  ))}
                </TabsContent>
                <TabsContent value="access">
                  <p className="detail-hint">
                    {selected.root_type === 'public'
                      ? 'Everyone in your organisation can read this entry.'
                      : 'Access may also come from ownership, tags, inherited grants, or organisation administration.'}
                  </p>
                  {selected.effective_access.includes('manage_permissions') && (
                    <Button
                      variant="outline"
                      onClick={() => edit('share', selected)}
                    >
                      <Plus size={15} /> Share with a member
                    </Button>
                  )}
                  {grants.map((grant) => (
                    <div className="detail-record" key={grant.id}>
                      <div>
                        <strong>
                          {grant.principal.type}:{grant.principal.id}
                        </strong>
                        <p>{grant.access.join(', ')}</p>
                      </div>
                      {selected.effective_access.includes(
                        'manage_permissions',
                      ) && (
                        <Button
                          variant="outline"
                          onClick={async () => {
                            try {
                              await api(
                                '/entries/' +
                                  selected.id +
                                  '/permissions/' +
                                  grant.id,
                                'DELETE',
                              );
                              setGrants((v) =>
                                v.filter((g) => g.id !== grant.id),
                              );
                            } catch (e) {
                              fail(e);
                            }
                          }}
                        >
                          Revoke
                        </Button>
                      )}
                    </div>
                  ))}
                  {!grants.length && !detailLoading && (
                    <p className="detail-hint">No explicit grants.</p>
                  )}
                </TabsContent>
                <TabsContent value="activity">
                  {activity.map((item, i) => (
                    <div className="detail-record" key={i}>
                      <div>
                        <strong>
                          {item.action
                            .replace(/^entry\./, '')
                            .replace(/\.v\d+$/, '')
                            .replaceAll('_', ' ')}
                        </strong>
                        <p>
                          {item.actor.type}:{item.actor.id}
                          {item.app_id ? ' · ' + item.app_id : ''}
                        </p>
                        <p>{new Date(item.occurred_at).toLocaleString()}</p>
                      </div>
                    </div>
                  ))}
                  {!activity.length && !detailLoading && (
                    <p>No recorded activity.</p>
                  )}
                </TabsContent>
              </Tabs>
            </div>
          )}
        </SheetContent>
      </Sheet>
    </SidebarProvider>
  );
}
