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
  Trash2,
  Search,
  Plus,
  Upload,
  ArrowUpRight,
  Download,
  LogOut,
  RefreshCw,
  Link as LinkIcon,
  X,
  ChevronDown,
  Hourglass,
  BriefcaseBusiness,
  LayoutGrid,
  List,
  SlidersHorizontal,
  ArrowLeft,
  ArrowRight,
  Check,
  LoaderCircle,
} from 'lucide-react';
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
import { Checkbox } from '@/components/ui/checkbox';
import { Button } from '@/components/ui/button';
import {
  Popover,
  PopoverTrigger,
  PopoverContent,
} from '@/components/ui/popover';
import FileShelf, { type FileShelfAction } from './file-shelf';
import FolderPicker from './folder-picker';
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
  browserUrl,
  returnToProduction,
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
import {
  DEFAULT_DURATION,
  DurationPicker,
  Lifetime,
  absoluteTime,
  durationMinutes,
  spokenDuration,
  type Duration,
} from './lifetime';
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
  /** An expiring share lasts this long, then only its own access ends. */
  expiring?: Duration;
};
type Usage = {
  storage: { used_bytes: number; limit_bytes: number; remaining_bytes: number };
  daily_uploads: { used_bytes: number; limit_bytes: number };
};
type Version = {
  id: string;
  number: number;
  sha256: string | null;
  source: string;
  size: number;
  created_at: string;
  created_by: { id: string };
};
type Activity = {
  action: string;
  actor_type: string;
  actor_id: string;
  metadata: Record<string, unknown>;
  app_id: string | null;
  occurred_at: string;
};
type Grant = {
  id: string;
  principal: { type: string; id: string };
  access: string[];
  inherit: boolean;
  /** When an expiring share ends; absent for a permanent share. */
  expires_at?: string | null;
};
type LinkAccess = {
  can_manage: boolean;
  enabled: boolean;
  effective: boolean;
  inherited_from: string | null;
  url: string | null;
  /** When this entry's own expiring link stops working. */
  expires_at?: string | null;
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
  const [versionCursor, setVersionCursor] = useState<string | null>(null);
  const [logCursor, setLogCursor] = useState<string | null>(null);
  const [linkAccess, setLinkAccess] = useState<LinkAccess | null>(null);
  const linkIntent = useRef<{
    id: string;
    action: string;
    operation: string;
  } | null>(null);
  // A chosen expiring time for the link: before it is on, or while changing it.
  const [linkExpiring, setLinkExpiring] = useState<Duration | null>(null),
    [grantChange, setGrantChange] = useState<{
      id: string;
      duration: Duration;
    } | null>(null),
    [selfDestructUpload, setSelfDestructUpload] = useState<{
      file: File | null;
      duration: Duration;
    } | null>(null);
  const [versions, setVersions] = useState<Version[]>([]),
    [grants, setGrants] = useState<Grant[]>([]),
    [grantCursor, setGrantCursor] = useState<string | null>(null),
    [activity, setActivity] = useState<Activity[]>([]),
    [detailError, setDetailError] = useState(''),
    [detailLoading, setDetailLoading] = useState(false);
  const [uploadProgress, setUploadProgress] = useState<number | null>(null),
    [retryUpload, setRetryUpload] = useState(false);
  useEffect(() => {
    if (uploadProgress === null && !retryUpload) return;
    const warnBeforeLeaving = (event: BeforeUnloadEvent) => {
      event.preventDefault();
    };
    window.addEventListener('beforeunload', warnBeforeLeaving);
    return () => window.removeEventListener('beforeunload', warnBeforeLeaving);
  }, [uploadProgress, retryUpload]);
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
  const [view, setView] = useState<'grid' | 'list'>('grid');
  const [focusedFile, setFocusedFile] = useState<Entry | null>(null);
  const [addOpen, setAddOpen] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [destination, setDestination] = useState<{
    files: File[];
    move?: Entry;
    moveOperations?: Map<string, string>;
  } | null>(null);
  const [uploadName, setUploadName] = useState('');
  const [uploadError, setUploadError] = useState('');
  const [uploadRejected, setUploadRejected] = useState(false);
  const uploadXhr = useRef<XMLHttpRequest | null>(null);
  const uploadLive = useRef(true);
  const [uploadCount, setUploadCount] = useState(0);
  const [recipientType, setRecipientType] = useState<
    'email' | 'c' | 'si' | 'tag'
  >('email');
  const searchInput = useRef<HTMLInputElement>(null);
  const searchReturn = useRef({ path: '', scope: 'files' as Scope });
  const dragDepth = useRef(0);
  const uploadQueue = useRef<{ file: File; parent: string }[]>([]);
  const uploadTarget = useRef<string | null>(null);
  const refreshCurrent = useRef<() => Promise<void>>(async () => {});
  useEffect(() => {
    const stored = localStorage.getItem('briefcase-file-view');
    // eslint-disable-next-line react/react-compiler -- Restore a local presentation preference after hydration.
    if (stored === 'list' || stored === 'grid') setView(stored);
    const shortcuts = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        searchInput.current?.focus();
        searchInput.current?.select();
      }
    };
    window.addEventListener('keydown', shortcuts);
    return () => window.removeEventListener('keydown', shortcuts);
  }, []);
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
      selfDestruct?: number;
    } | null>(null),
    generation = useRef(0);
  useEffect(() => {
    uploadLive.current = true;
    return () => {
      uploadLive.current = false;
      uploadQueue.current = [];
      const xhr = uploadXhr.current;
      if (xhr) {
        xhr.onload = null;
        xhr.onerror = null;
        xhr.ontimeout = null;
        xhr.upload.onprogress = null;
        xhr.abort();
      }
    };
  }, []);
  useEffect(() => {
    const deselect = (event: KeyboardEvent) => {
      if (
        event.key === 'Escape' &&
        !document.querySelector(
          '[role="dialog"], [role="menu"], [data-slot="popover-content"]',
        )
      )
        setFocusedFile(null);
    };
    window.addEventListener('keydown', deselect);
    return () => window.removeEventListener('keydown', deselect);
  }, []);
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
  // Actions inside the details sheet report there, beside what they changed.
  function detailFail(e: unknown) {
    if (e instanceof ApiError && e.status === 401) {
      onSignOut();
      return;
    }
    setDetailError(
      e instanceof Error ? e.message : 'The request could not be completed.',
    );
  }
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
    refreshCurrent.current = load;
  }, [load]);
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
    setAddOpen(false);
    setFocusedFile(null);
    setAdvanced(false);
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
    setAddOpen(false);
    setRecipientType('email');
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
        const expiring = editor.expiring
          ? durationMinutes(editor.expiring)
          : undefined;
        if (expiring === null)
          throw new Error('Choose a time between 1 minute and 30 days.');
        const recipient = /^(c|si|email|tag):/.test(editor.value.trim())
          ? editor.value.trim()
          : recipientType + ':' + editor.value.trim();
        const [type, ...name] = recipient.split(':');
        if (!['c', 'si', 'email', 'tag'].includes(type) || !name.join(':'))
          throw new Error('Use c:ID, si:ID, email:address, or tag:tag.');
        await api('/entries/' + editor.entry!.id + '/invitations', 'POST', {
          principal: {
            type: type === 'c' ? 'carbon' : type === 'si' ? 'silicon' : type,
            id: type === 'c' || type === 'si' ? recipient : name.join(':'),
          },
          // An expiring share only ever lets people view and download.
          access: expiring ? ['read'] : rights,
          expires_in_minutes: expiring,
          operation_id: editor.operation,
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
      await refreshed(
        confirm.self_destruct_at
          ? 'Deleted ' + confirm.name + ' permanently.'
          : 'Moved to the bin. Recoverable for 45 days.',
      );
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
  async function upload(
    file?: File,
    selfDestruct?: number,
    targetPath?: string,
  ) {
    if (file)
      uploadIntent.current = {
        file,
        parent: targetPath ?? uploadTarget.current ?? path,
        operation: crypto.randomUUID(),
        selfDestruct,
      };
    const intent = uploadIntent.current;
    if (!intent || !uploadLive.current) return;
    setUploadError('');
    setUploadRejected(false);
    setUploadName(intent.file.name);
    setUploadCount(uploadQueue.current.length + 1);
    setUploadProgress(0);
    setRetryUpload(false);
    setError('');
    const p = new URLSearchParams({
      parent: intent.parent,
      name: intent.file.name,
      content_type: intent.file.type || 'application/octet-stream',
      operation_id: intent.operation,
    });
    if (intent.selfDestruct)
      p.set('self_destruct_minutes', String(intent.selfDestruct));
    const xhr = new XMLHttpRequest();
    uploadXhr.current = xhr;
    xhr.open('POST', browserUrl('/browser/upload?' + p));
    xhr.setRequestHeader('X-Briefcase-Browser', '1');
    xhr.setRequestHeader('X-Briefcase-Organization', session.org);
    xhr.timeout = 1800000;
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable)
        setUploadProgress(Math.round((e.loaded / e.total) * 100));
    };
    const failed = (message: string, rejected = false) => {
      if (!uploadLive.current) return;
      setUploadProgress(null);
      setRetryUpload(true);
      setUploadError(message);
      setUploadRejected(rejected);
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
      if (!uploadLive.current) return;
      if (xhr.status === 401) {
        onSignOut();
        return;
      }
      let result;
      try {
        result = JSON.parse(xhr.responseText);
      } catch {
        failed('Unable to read the upload result. Retry the same upload.');
        return;
      }
      if (xhr.status < 200 || xhr.status >= 300) {
        const message = result.error?.message || 'Upload failed.';
        const rejectedConflict =
          xhr.status === 409 &&
          (message.startsWith('Self destruct only applies to new files') ||
            message.startsWith('The destination changed during upload'));
        failed(
          message,
          rejectedConflict ||
            [400, 403, 404, 413, 415, 422].includes(xhr.status),
        );
        return;
      }
      uploadIntent.current = null;
      const next = uploadQueue.current.shift();
      if (next) void upload(next.file, undefined, next.parent);
      else {
        setUploadProgress(null);
        setUploadCount(0);
        uploadTarget.current = null;
      }
      void refreshCurrent.current();
      api<Usage>('/usage').then(setUsage).catch(fail);
      setNotice(
        'Uploaded ' +
          intent.file.name +
          (result.self_destruct_at
            ? '. It will be deleted for good on ' +
              absoluteTime(result.self_destruct_at)
            : '') +
          '.',
      );
    };
    xhr.send(intent.file);
  }
  async function changeLink(
    entry: Entry,
    change: { enabled: boolean; minutes?: number },
  ) {
    const action = !change.enabled
      ? 'off'
      : change.minutes
        ? 'expiring:' + change.minutes
        : 'on';
    if (
      !linkIntent.current ||
      linkIntent.current.id !== entry.id ||
      linkIntent.current.action !== action
    )
      linkIntent.current = {
        id: entry.id,
        action,
        operation: crypto.randomUUID(),
      };
    setWorking(true);
    setDetailError('');
    try {
      const result = await api<LinkAccess>(
        '/entries/' + entry.id + '/link-access',
        'PUT',
        {
          enabled: change.enabled,
          expires_in_minutes: change.minutes,
          operation_id: linkIntent.current.operation,
        },
      );
      setLinkAccess(result);
      setLinkExpiring(null);
      linkIntent.current = null;
    } catch (e) {
      detailFail(e);
    } finally {
      setWorking(false);
    }
  }
  async function refreshLink(entry: Entry) {
    try {
      setLinkAccess(
        await api<LinkAccess>('/entries/' + entry.id + '/link-access'),
      );
    } catch (e) {
      detailFail(e);
    }
  }
  async function changeExpiring(
    entry: Entry,
    grant: Grant,
    change: { minutes: number } | { permanent: true },
  ) {
    setWorking(true);
    setDetailError('');
    try {
      const result = await api<Grant>(
        '/entries/' + entry.id + '/invitations/' + grant.id,
        'PATCH',
        {
          ...('minutes' in change
            ? { expires_in_minutes: change.minutes }
            : { permanent: true }),
          operation_id: crypto.randomUUID(),
        },
      );
      // Made permanent, a share can fold into the recipient's existing grant.
      setGrants((value) => {
        const next = value.map((g) => (g.id === grant.id ? result : g));
        return next.filter(
          (g, i) => next.findIndex((other) => other.id === g.id) === i,
        );
      });
      setGrantChange(null);
    } catch (e) {
      if (e instanceof ApiError && e.status === 404)
        setGrants((value) => value.filter((g) => g.id !== grant.id));
      detailFail(e);
    } finally {
      setWorking(false);
    }
  }
  async function keepFile(entry: Entry) {
    setWorking(true);
    setDetailError('');
    try {
      await api('/entries/' + entry.id + '/self-destruct', 'DELETE');
      const kept = { ...entry, self_destruct_at: null };
      setSelected(kept);
      setItems((value) =>
        value.map((item) => (item.id === entry.id ? kept : item)),
      );
      setNotice('Kept ' + entry.name + '. It will no longer self destruct.');
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) {
        // Its timer already stopped: show the file as it is now.
        const current = await api<Entry>('/entries/' + entry.id).catch(
          () => null,
        );
        if (current) {
          setSelected(current);
          setItems((value) =>
            value.map((item) => (item.id === entry.id ? current : item)),
          );
        }
      }
      detailFail(e);
    } finally {
      setWorking(false);
    }
  }
  // The file is already gone upstream; drop it rather than offer dead actions.
  function selfDestructed(entry: Entry) {
    setItems((value) => value.filter((item) => item.id !== entry.id));
    if (selected?.id === entry.id) {
      closeDetails();
      setNotice(entry.name + ' self-destructed and was deleted for good.');
    }
  }
  useEffect(() => {
    let current = true;
    const abort = new AbortController();
    // eslint-disable-next-line react/react-compiler -- Clear the previous entry's preview before starting a cancellable request for the new selection.
    setDetailError('');
    setPreview(null);
    setVersions([]);
    setVersionCursor(null);
    setLogCursor(null);
    setLinkAccess(null);
    setLinkExpiring(null);
    setGrantChange(null);
    setGrants([]);
    setGrantCursor(null);
    setActivity([]);
    if (!selected || scope === 'bin') {
      setDetailLoading(false);
      return;
    }
    setDetailLoading(true);
    const run = async () => {
      try {
        if (tab === 'versions') {
          const v = await api<{ items: Version[]; next_cursor: string | null }>(
            '/entries/' + selected.id + '/versions',
          );
          if (current) {
            setVersions(v.items);
            setVersionCursor(v.next_cursor);
          }
        } else if (tab === 'access') {
          const link = await api<LinkAccess>(
            '/entries/' + selected.id + '/link-access',
          );
          if (current) setLinkAccess(link);
          if (selected.effective_access.includes('manage_permissions')) {
            const g = await api<{ items: Grant[]; next_cursor: string | null }>(
              '/entries/' + selected.id + '/invitations',
            );
            if (current) {
              setGrants(g.items);
              setGrantCursor(g.next_cursor);
            }
          }
        } else if (tab === 'activity') {
          const a = await api<{
            items: Activity[];
            next_cursor: string | null;
          }>('/entries/' + selected.id + '/logs');
          if (current) {
            setActivity(a.items);
            setLogCursor(a.next_cursor);
          }
        } else if (
          selected.type === 'file' &&
          selected.effective_access.includes('read')
        ) {
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
  const uploadBusy = uploadProgress !== null || retryUpload;
  function beginUpload(files: File[], target?: string) {
    if (!files.length || uploadBusy || uploadIntent.current) return;
    if (!target && !canUpload) {
      setDestination({ files });
      return;
    }
    const parentPath = target ?? path;
    uploadQueue.current = files
      .slice(1)
      .map((file) => ({ file, parent: parentPath }));
    void upload(files[0], undefined, parentPath);
  }
  function chooseFiles() {
    setAddOpen(false);
    if (canUpload) {
      uploadTarget.current = path;
      fileInput.current?.click();
    } else setDestination({ files: [] });
  }
  function searchFiles(event: SubmitEvent) {
    event.preventDefault();
    if (!query.trim()) return;
    if (scope !== 'search') searchReturn.current = { path, scope };
    navigate('', 'search');
    setSearch(query.trim());
  }
  function leaveSearch() {
    setQuery('');
    setSearch('');
    navigate(searchReturn.current.path, searchReturn.current.scope);
    searchInput.current?.focus();
  }
  function fileAction(action: FileShelfAction, entry: Entry) {
    if (action === 'download')
      window.location.assign(
        browserUrl('/browser/entries/' + entry.id + '/content?download=true'),
      );
    else if (action === 'delete') {
      setError('');
      setConfirm(entry);
    } else if (action === 'restore') void restore(entry);
    else if (action === 'details') {
      setSelected(entry);
      setTab('activity');
    } else if (action === 'move')
      setDestination({ files: [], move: entry, moveOperations: new Map() });
    else edit(action, entry);
  }
  // Self destruct is refused for a new version of a file already listed here.
  const selfDestructName = selfDestructUpload?.file?.name.trim();
  const selfDestructClash =
    !!selfDestructName &&
    items.some(
      (item) => item.type === 'file' && item.name === selfDestructName,
    );
  const contentUrl = selected
    ? browserUrl('/browser/entries/' + selected.id + '/content')
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
    <div className="worktable">
      {session.testing && (
        <aside className="testing-view-banner" aria-label="Testing environment">
          <div>
            <strong>
              Testing environment · {session.test_environment?.name}
            </strong>
            <p>
              Files and changes stay in this environment. Signed in as{' '}
              {session.actor.public_id}.
            </p>
          </div>
          <Button
            variant="outline"
            disabled={uploadBusy}
            onClick={returnToProduction}
          >
            Return to production
          </Button>
        </aside>
      )}
      <header className="worktable-header">
        <div className="worktable-identity">
          <button
            className="worktable-brand"
            onClick={() => navigate('')}
            aria-label="Briefcase, all files"
          >
            <BriefcaseBusiness size={26} strokeWidth={1.7} />
            <span>briefcase</span>
          </button>
          <button
            className="workspace-switch"
            disabled={uploadBusy}
            title={
              uploadBusy
                ? 'Finish or retry your upload before switching workspaces'
                : 'Switch workspace'
            }
            onClick={onChooseOrganization}
          >
            {session.org}
            <ChevronDown size={15} />
          </button>
        </div>
        <form className="worktable-search" onSubmit={searchFiles}>
          <Search size={19} aria-hidden="true" />
          <input
            ref={searchInput}
            aria-label="Search filenames and contents"
            placeholder="Find a file"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          {query ? (
            <button type="submit" aria-label="Search files">
              <ArrowRight size={17} />
            </button>
          ) : (
            <kbd title="Command or Control K">⌘ K</kbd>
          )}
        </form>
        <div className="worktable-account">
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
          <Popover>
            <PopoverTrigger
              render={
                <Button
                  variant="ghost"
                  className="account-trigger"
                  aria-label="Account and workspace"
                />
              }
            >
              <span>
                {session.actor.public_id
                  .replace(/^[^:]+:/, '')
                  .slice(0, 2)
                  .toUpperCase()}
              </span>
            </PopoverTrigger>
            <PopoverContent align="end" className="account-popover">
              <strong>{session.actor.public_id}</strong>
              <small>
                {session.testing
                  ? 'Testing environment'
                  : 'Organisation workspace'}
              </small>
              {usage && (
                <small>
                  {bytes(usage.storage.used_bytes)} used ·{' '}
                  {bytes(usage.storage.remaining_bytes)} available
                </small>
              )}
              <Button
                variant="ghost"
                disabled={uploadBusy}
                onClick={onChooseOrganization}
              >
                Switch workspace <ChevronDown size={15} />
              </Button>
              <TestingEnvironments
                session={session}
                onUnauthorized={onSignOut}
                disabled={uploadBusy}
              />
              {uploadBusy && (
                <small>
                  Finish or resolve your upload before leaving this workspace.
                </small>
              )}
              <a
                href="https://docs.briefcase.teamofsilicons.com/"
                target="_blank"
                rel="noreferrer"
              >
                Help & documentation <ArrowUpRight size={15} />
              </a>
              <Button
                variant="ghost"
                disabled={uploadBusy}
                onClick={async () => {
                  try {
                    await api('/session', 'DELETE');
                    onSignOut();
                  } catch (e) {
                    fail(e);
                  }
                }}
              >
                <LogOut size={16} /> Sign out
              </Button>
            </PopoverContent>
          </Popover>
        </div>
      </header>
      {/* eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions -- Native file drop complements the keyboard-accessible Add files button. */}
      <main
        className={
          'file-surface worktable-surface' + (dragging ? ' is-dragging' : '')
        }
        onDragEnter={(event) => {
          if (Array.from(event.dataTransfer.types).includes('Files')) {
            event.preventDefault();
            dragDepth.current++;
            setDragging(true);
          }
        }}
        onDragOver={(event) => {
          if (Array.from(event.dataTransfer.types).includes('Files')) {
            event.preventDefault();
            event.dataTransfer.dropEffect = uploadBusy ? 'none' : 'copy';
          }
        }}
        onDragLeave={(event) => {
          event.preventDefault();
          dragDepth.current = Math.max(0, dragDepth.current - 1);
          if (!dragDepth.current) setDragging(false);
        }}
        onDrop={(event) => {
          event.preventDefault();
          dragDepth.current = 0;
          setDragging(false);
          if (!uploadBusy) beginUpload(Array.from(event.dataTransfer.files));
        }}
      >
        <nav className="breadcrumbs" aria-label="Folder path">
          <button onClick={() => navigate('')}>{session.org}</button>
          {path
            .split('/')
            .filter(Boolean)
            .map((part, i, all) => (
              <span key={i}>
                <span aria-hidden="true">/</span>
                <button onClick={() => navigate(all.slice(0, i + 1).join('/'))}>
                  {part}
                </button>
              </span>
            ))}
        </nav>
        <div className="file-title">
          <div>
            <h1>{title}</h1>
            <p className="workspace-subtitle">
              {scope === 'bin'
                ? 'A second chance. Restore files within 45 days.'
                : scope === 'search'
                  ? 'Files matching “' + search + '”'
                  : scope === 'recent'
                    ? 'The latest work, within reach.'
                    : path
                      ? 'A little space for your work.'
                      : 'Everything has its place.'}
            </p>
          </div>
          <Popover open={addOpen} onOpenChange={setAddOpen}>
            <PopoverTrigger
              render={
                <Button
                  className="add-files"
                  disabled={scope === 'bin' || uploadBusy}
                />
              }
            >
              <Plus size={21} /> Add files
            </PopoverTrigger>
            <PopoverContent align="end" sideOffset={12} className="add-popover">
              <p>Add to {canUpload ? title : 'a folder'}</p>
              <button onClick={chooseFiles}>
                <Upload size={21} />
                <span>
                  Upload files<small>Choose from your device</small>
                </span>
              </button>
              <button
                disabled={!canCreateFolder || working}
                onClick={() => edit('folder')}
              >
                <Folder size={21} />
                <span>
                  New folder<small>Make a little room</small>
                </span>
              </button>
              <button
                disabled={!canUpload}
                onClick={() => {
                  setAddOpen(false);
                  setError('');
                  uploadTarget.current = path;
                  setSelfDestructUpload({
                    file: null,
                    duration: DEFAULT_DURATION,
                  });
                }}
              >
                <Hourglass size={21} />
                <span>
                  Self-destructing file
                  <small>Automatically delete after a set time</small>
                </span>
              </button>
              <small className="add-hint">
                You can also drop files into this space.
              </small>
            </PopoverContent>
          </Popover>
          <input
            ref={fileInput}
            type="file"
            multiple
            hidden
            onChange={(event) => {
              const files = Array.from(event.target.files ?? []);
              beginUpload(files, uploadTarget.current ?? undefined);
              event.currentTarget.value = '';
            }}
          />
        </div>
        <div className="worktable-navigation">
          <nav className="space-pills" aria-label="File spaces">
            {[
              { label: 'All files', p: '', s: 'files' as Scope },
              { label: 'Recent', p: '', s: 'recent' as Scope },
              { label: 'Public', p: 'public', s: 'files' as Scope },
              {
                label: 'Private',
                p: 'private/' + session.actor.public_id,
                s: 'files' as Scope,
              },
              ...roots
                .filter((root) => root.root_type === 'tag')
                .map((root) => ({
                  label: root.name,
                  p: root.path,
                  s: 'files' as Scope,
                })),
            ].map((item) => (
              <button
                key={item.label + item.p}
                className={
                  scope === item.s &&
                  (path === item.p ||
                    (!!item.p && path.startsWith(item.p + '/')))
                    ? 'active'
                    : ''
                }
                aria-current={
                  scope === item.s &&
                  (path === item.p ||
                    (!!item.p && path.startsWith(item.p + '/')))
                    ? 'page'
                    : undefined
                }
                onClick={() => navigate(item.p, item.s)}
              >
                {item.label}
              </button>
            ))}
          </nav>
          <div className="view-controls">
            {scope === 'search' && (
              <Button variant="ghost" onClick={leaveSearch}>
                <ArrowLeft size={15} /> Back
              </Button>
            )}
            {scope === 'files' && (
              <Button
                variant="ghost"
                size="icon"
                aria-label="Filter files"
                aria-expanded={advanced}
                onClick={() => setAdvanced((value) => !value)}
              >
                <SlidersHorizontal size={17} />
              </Button>
            )}
            <fieldset className="view-switch" aria-label="File view">
              <button
                aria-label="Grid view"
                aria-pressed={view === 'grid'}
                onClick={() => {
                  setView('grid');
                  localStorage.setItem('briefcase-file-view', 'grid');
                }}
              >
                <LayoutGrid size={17} />
              </button>
              <button
                aria-label="List view"
                aria-pressed={view === 'list'}
                onClick={() => {
                  setView('list');
                  localStorage.setItem('briefcase-file-view', 'list');
                }}
              >
                <List size={18} />
              </button>
            </fieldset>
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
              You might not be seeing all the contents of this folder. This is a
              permission-based folder.
            </p>
          )}
        {error && (
          <div role="alert" className="error-box">
            {error}
          </div>
        )}
        {notice && (
          <output className="notice worktable-toast">
            <Check size={18} />
            <span>{notice}</span>
            <button aria-label="Dismiss" onClick={() => setNotice('')}>
              <X size={16} />
            </button>
          </output>
        )}
        {(uploadProgress !== null || retryUpload) && (
          <output className="upload-capsule">
            <span className="upload-symbol">
              <Upload size={24} />
            </span>
            <span className="upload-caption">
              <strong>{uploadName}</strong>
              {retryUpload ? (
                <>
                  <span>{uploadError}</span>
                  <span className="upload-recovery">
                    <Button size="sm" onClick={() => void upload()}>
                      Retry same upload
                    </Button>
                    {uploadRejected && (
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => {
                          uploadQueue.current = [];
                          uploadIntent.current = null;
                          uploadTarget.current = null;
                          setRetryUpload(false);
                          setUploadError('');
                          setUploadCount(0);
                        }}
                      >
                        Dismiss
                      </Button>
                    )}
                  </span>
                </>
              ) : (
                <>
                  <span>
                    {uploadProgress === 100
                      ? 'Finishing upload…'
                      : `Uploading · ${uploadProgress}%`}
                    {uploadCount > 1 ? ` · ${uploadCount - 1} waiting` : ''}
                  </span>
                  <progress value={uploadProgress ?? 0} max={100} />
                </>
              )}
            </span>
            {!retryUpload && <LoaderCircle size={19} className="is-spinning" />}
          </output>
        )}
        <FileShelf
          entries={items}
          mode={view}
          scope={scope}
          selectedId={focusedFile?.id}
          onSelect={(entry) =>
            setFocusedFile((previous) =>
              previous?.id === entry.id ? null : entry,
            )
          }
          onOpen={open}
          onAction={fileAction}
          onSelfDestruct={selfDestructed}
        />
        {(loading || routeLoading) && (
          <output className="loading-files">
            <LoaderCircle size={22} className="is-spinning" /> Gathering your
            files…
          </output>
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
        {dragging && (
          <div className="drop-overlay">
            <Upload size={42} />
            <strong>
              {uploadBusy
                ? 'An upload is already in progress'
                : canUpload
                  ? 'Let go. We’ll put it here.'
                  : 'Drop your files, then choose a folder.'}
            </strong>
            <span>{canUpload ? path : 'Choose where your files belong'}</span>
          </div>
        )}
      </main>
      <footer className="worktable-footer">
        <span className="drop-hint">
          <Upload size={18} />{' '}
          {uploadBusy
            ? 'Your upload stays here while you browse.'
            : 'Drop files into your workspace.'}
        </span>
        <div>
          <Button
            variant="ghost"
            onClick={() => navigate('', 'bin')}
            aria-current={scope === 'bin' ? 'page' : undefined}
          >
            <Trash2 size={17} /> Bin
          </Button>
          <OrganizationSettings session={session} onUnauthorized={onSignOut} />
        </div>
      </footer>
      {destination && (
        <FolderPicker
          title={
            destination.move
              ? 'Move ' + destination.move.name
              : 'Where should these files go?'
          }
          description={
            destination.move
              ? 'Choose a folder you can add content to.'
              : 'Pick a home for your upload.'
          }
          excludePath={
            destination.move?.type === 'folder'
              ? destination.move.path
              : undefined
          }
          chooseLabel={destination.move ? 'Move here' : 'Choose this folder'}
          pendingLabel={destination.move ? 'Moving…' : 'Choosing…'}
          onClose={() => setDestination(null)}
          onChoose={(folder) => {
            const choice = destination;
            if (choice.move) {
              const entry = choice.move;
              const key = entry.id + ':' + folder.id;
              const operations = choice.moveOperations!;
              let operation = operations.get(key);
              if (!operation) {
                operation = crypto.randomUUID();
                operations.set(key, operation);
              }
              return api<Entry>('/entries/' + entry.id, 'PATCH', {
                parent_id: folder.id,
                operation_id: operation,
              })
                .then(async () => {
                  setDestination(null);
                  setFocusedFile(null);
                  setSelected(null);
                  await refreshed('Moved ' + entry.name + '.');
                })
                .catch((reason: unknown) => {
                  if (reason instanceof ApiError && reason.status === 401)
                    onSignOut();
                  throw reason;
                });
            }
            setDestination(null);
            if (choice.files.length) beginUpload(choice.files, folder.path);
            else {
              uploadTarget.current = folder.path;
              fileInput.current?.click();
            }
          }}
        />
      )}

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
                ? 'Choose who can access this file, and for how long.'
                : editor?.kind === 'move'
                  ? 'Confirm your destination below.'
                  : 'Give it a name that’s easy to find.'}
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
                ? 'Person or team'
                : editor?.kind === 'move'
                  ? 'Destination folder path'
                  : 'Name'}
            </label>
            {editor?.kind === 'share' && (
              <Select
                value={recipientType}
                disabled={working}
                onValueChange={(value) => {
                  if (
                    value === 'email' ||
                    value === 'c' ||
                    value === 'si' ||
                    value === 'tag'
                  )
                    setRecipientType(value);
                }}
              >
                <SelectTrigger aria-label="Recipient type">
                  <SelectValue>
                    {recipientType === 'email'
                      ? 'Email address'
                      : recipientType === 'c'
                        ? 'Carbon ID'
                        : recipientType === 'si'
                          ? 'Silicon ID'
                          : 'Team tag'}
                  </SelectValue>
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="email">Email address</SelectItem>
                  <SelectItem value="c">Carbon ID</SelectItem>
                  <SelectItem value="si">Silicon ID</SelectItem>
                  <SelectItem value="tag">Team tag</SelectItem>
                </SelectContent>
              </Select>
            )}
            <Input
              id="editor-value"
              disabled={working}
              required
              maxLength={255}
              placeholder={
                editor?.kind === 'share'
                  ? recipientType === 'email'
                    ? 'name@company.com'
                    : recipientType === 'tag'
                      ? 'design'
                      : 'Enter an ID'
                  : undefined
              }
              value={editor?.value || ''}
              onChange={(e) =>
                setEditor((v) => (v ? { ...v, value: e.target.value } : null))
              }
            />
            {editor?.kind === 'share' && (
              <div className="rights">
                {(editor.entry?.type === 'folder'
                  ? ['read', 'write', 'update']
                  : ['read', 'update']
                ).map((right) => (
                  <label key={right}>
                    <Checkbox
                      checked={
                        right === 'read' ||
                        (!editor.expiring && rights.includes(right))
                      }
                      disabled={
                        working || right === 'read' || !!editor.expiring
                      }
                      onCheckedChange={(checked) =>
                        setRights((v) =>
                          checked
                            ? [...v, right]
                            : v.filter((r) => r !== right),
                        )
                      }
                    />
                    {right === 'write'
                      ? 'Add files'
                      : right === 'read'
                        ? 'View & download'
                        : 'Edit content'}
                  </label>
                ))}
              </div>
            )}
            {editor?.kind === 'share' && (
              <div className="lifetime-fields">
                <label
                  className="lifetime-toggle"
                  htmlFor="share-expiring-toggle"
                >
                  <Checkbox
                    id="share-expiring-toggle"
                    checked={!!editor.expiring}
                    disabled={working}
                    onCheckedChange={(checked) =>
                      setEditor((v) =>
                        v
                          ? {
                              ...v,
                              expiring: checked ? DEFAULT_DURATION : undefined,
                            }
                          : null,
                      )
                    }
                  />
                  Expires after
                </label>
                {editor.expiring ? (
                  <>
                    <DurationPicker
                      id="share-expiring"
                      value={editor.expiring}
                      disabled={working}
                      onChange={(expiring) =>
                        setEditor((v) => (v ? { ...v, expiring } : null))
                      }
                    />
                    <p className="field-note">
                      View and download only. This shared access ends{' '}
                      {(() => {
                        const minutes = durationMinutes(editor.expiring);
                        return minutes
                          ? spokenDuration(minutes) + ' from now'
                          : 'at the time you choose';
                      })()}
                      , and no one is notified. Other access they have stays.
                    </p>
                  </>
                ) : (
                  <p className="field-note">
                    Access stays until you revoke it. Your file stays in place.
                  </p>
                )}
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
              disabled={
                working ||
                (editor?.rootType === 'tag' && !editor.tag) ||
                (!!editor?.expiring &&
                  durationMinutes(editor.expiring) === null)
              }
            >
              {working
                ? 'Saving…'
                : editor?.kind === 'share'
                  ? editor.expiring
                    ? 'Share for ' +
                      (durationMinutes(editor.expiring)
                        ? spokenDuration(durationMinutes(editor.expiring)!)
                        : 'a limited time')
                    : 'Share file'
                  : editor?.kind === 'folder'
                    ? 'Create folder'
                    : editor?.kind === 'move'
                      ? 'Move here'
                      : 'Save name'}
            </Button>
          </form>
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!selfDestructUpload}
        onOpenChange={(open) => {
          if (!open) setSelfDestructUpload(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Self-destructing upload</DialogTitle>
            <DialogDescription>
              Briefcase deletes the file for good when its time runs out. It
              never goes to the bin.
            </DialogDescription>
          </DialogHeader>
          {selfDestructUpload && (
            <form
              onSubmit={(e) => {
                e.preventDefault();
                const minutes = durationMinutes(selfDestructUpload.duration);
                if (!selfDestructUpload.file || !minutes) return;
                setSelfDestructUpload(null);
                void upload(selfDestructUpload.file, minutes);
              }}
            >
              <label htmlFor="self-destruct-file">File</label>
              <Input
                id="self-destruct-file"
                type="file"
                required
                onChange={(e) => {
                  const file = e.target.files?.[0] ?? null;
                  setSelfDestructUpload((v) => (v ? { ...v, file } : null));
                }}
              />
              <label htmlFor="self-destruct-duration">Delete it after</label>
              <DurationPicker
                id="self-destruct-duration"
                value={selfDestructUpload.duration}
                onChange={(duration) =>
                  setSelfDestructUpload((v) => (v ? { ...v, duration } : null))
                }
              />
              <p className="field-note">
                The timer starts when the upload finishes. Only new files can
                self destruct, and uploading a new version later won’t change
                the timer.
              </p>
              {selfDestructClash && (
                <p className="error-box" role="alert">
                  A file named {selfDestructUpload.file?.name} already exists
                  here. Self destruct only applies to new files: rename the file
                  or upload it without self destruct.
                </p>
              )}
              <Button
                type="submit"
                className="primary-action"
                disabled={
                  !selfDestructUpload.file ||
                  durationMinutes(selfDestructUpload.duration) === null ||
                  selfDestructClash
                }
              >
                Upload
                <Upload size={16} />
              </Button>
            </form>
          )}
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
              {confirm?.self_destruct_at
                ? `Delete “${confirm.name}” permanently?`
                : `Move “${confirm?.name}” to the bin?`}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {confirm?.self_destruct_at
                ? 'This file is set to self destruct, so deleting it is permanent. It won’t go to the bin and can’t be recovered.'
                : 'It will disappear from your files. You can recover it for 45 days. A folder’s contents move with it.'}
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
              {confirm?.self_destruct_at
                ? working
                  ? 'Deleting…'
                  : 'Delete permanently'
                : working
                  ? 'Moving…'
                  : 'Move to bin'}
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
        <SheetContent className="details-sheet file-details data-[side=right]:sm:max-w-2xl">
          <SheetHeader>
            <SheetTitle>{selected?.name}</SheetTitle>
            <SheetDescription>{selected?.path}</SheetDescription>
          </SheetHeader>
          {selected && (
            <div className="detail-body">
              <div className="detail-actions">
                {scope !== 'bin' &&
                  selected.effective_access.includes('manage_permissions') && (
                    <Button
                      onClick={() => {
                        const entry = selected;
                        closeDetails();
                        edit('share', entry);
                      }}
                    >
                      Share file <ArrowUpRight size={16} />
                    </Button>
                  )}
                {scope !== 'bin' &&
                  selected.effective_access.includes('read') && (
                    <a
                      className="download-link"
                      href={
                        contentUrl +
                        (contentUrl.includes('?') ? '&' : '?') +
                        'download=true'
                      }
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
              {selected.self_destruct_at && scope !== 'bin' && (
                <div className="self-destruct-callout">
                  <div>
                    <Lifetime
                      kind="self-destruct"
                      at={selected.self_destruct_at}
                      onElapsed={() => selfDestructed(selected)}
                    />
                    <p>
                      Briefcase deletes this file for good on{' '}
                      {absoluteTime(selected.self_destruct_at)}. It won’t go to
                      the bin. Its uploader, org admins and org owners can keep
                      it.
                    </p>
                  </div>
                  <Button
                    variant="outline"
                    disabled={working}
                    onClick={() => void keepFile(selected)}
                  >
                    Keep file
                  </Button>
                </div>
              )}
              <Tabs value={tab} onValueChange={setTab}>
                <TabsList>
                  <TabsTrigger value="preview">Preview</TabsTrigger>
                  <TabsTrigger value="access">Access</TabsTrigger>
                  {selected.type === 'file' && (
                    <TabsTrigger value="versions">Versions</TabsTrigger>
                  )}
                  <TabsTrigger value="activity">Activity</TabsTrigger>
                </TabsList>
                {detailError && <p className="error-box">{detailError}</p>}
                {detailLoading && <output>Loading…</output>}
                <TabsContent value="preview">
                  <div className="preview">
                    {scope === 'bin' ? (
                      <p>Restore this entry to open its content.</p>
                    ) : !selected.effective_access.includes('read') ? (
                      <p>You do not have permission to preview this file.</p>
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
                      <dd>
                        {selected.effective_access
                          .map(
                            (access) =>
                              ({
                                read: 'View & download',
                                write: 'Add files',
                                update: 'Edit content',
                                delete: 'Delete',
                                manage_permissions: 'Manage sharing',
                              })[access] || access,
                          )
                          .join(' · ')}
                      </dd>
                    </div>
                  </dl>
                </TabsContent>
                <TabsContent value="versions">
                  <p className="detail-hint">
                    Every version is retained. Restoring creates a new version.
                  </p>
                  {versions.map((version) => (
                    <div className="detail-record" key={version.id}>
                      <div>
                        <strong>Version {version.number}</strong>
                        <p>{version.source}</p>
                        {version.sha256 && (
                          <p title={version.sha256}>
                            SHA-256: {version.sha256.slice(0, 16)}…
                          </p>
                        )}
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
                  {versionCursor && (
                    <Button
                      variant="outline"
                      disabled={working}
                      onClick={async () => {
                        setWorking(true);
                        try {
                          const page = await api<{
                            items: Version[];
                            next_cursor: string | null;
                          }>(
                            '/entries/' +
                              selected.id +
                              '/versions?cursor=' +
                              encodeURIComponent(versionCursor),
                          );
                          setVersions((v) => [...v, ...page.items]);
                          setVersionCursor(page.next_cursor);
                        } catch (e) {
                          fail(e);
                        } finally {
                          setWorking(false);
                        }
                      }}
                    >
                      Older versions
                    </Button>
                  )}
                </TabsContent>
                <TabsContent value="access">
                  {linkAccess && (
                    <div className="detail-record">
                      <div>
                        <strong>Anyone with the link</strong>
                        <p>
                          {linkAccess.effective
                            ? 'Can view and download'
                            : 'Requires an authorized account'}
                        </p>
                        {linkAccess.enabled && linkAccess.expires_at && (
                          <Lifetime
                            kind="expiring"
                            at={linkAccess.expires_at}
                            onElapsed={() => void refreshLink(selected)}
                          />
                        )}
                        {linkAccess.effective && linkAccess.url && (
                          <div className="mt-3 flex min-w-0 flex-col items-start gap-2">
                            <a
                              className="break-all text-sm underline"
                              href={linkAccess.url}
                              target="_blank"
                              rel="noreferrer"
                            >
                              {linkAccess.url}
                            </a>
                            <Button
                              variant="outline"
                              onClick={async () => {
                                try {
                                  await navigator.clipboard.writeText(
                                    linkAccess.url!,
                                  );
                                  setNotice('Share link copied.');
                                } catch (e) {
                                  fail(e);
                                }
                              }}
                            >
                              <LinkIcon size={15} /> Copy share link
                            </Button>
                          </div>
                        )}
                        {linkAccess.inherited_from && (
                          <p>
                            Access is inherited from a shared parent folder.
                            Change the parent to remove inherited access.
                          </p>
                        )}
                        {linkAccess.can_manage && !linkAccess.enabled && (
                          <div className="lifetime-fields">
                            <label
                              className="lifetime-toggle"
                              htmlFor="link-expiring-toggle"
                            >
                              <Checkbox
                                id="link-expiring-toggle"
                                checked={!!linkExpiring}
                                disabled={working}
                                onCheckedChange={(checked) =>
                                  setLinkExpiring(
                                    checked ? DEFAULT_DURATION : null,
                                  )
                                }
                              />
                              Expires after
                            </label>
                            {linkExpiring && (
                              <>
                                <DurationPicker
                                  id="link-expiring"
                                  value={linkExpiring}
                                  disabled={working}
                                  onChange={setLinkExpiring}
                                />
                                <p className="field-note">
                                  The link stops working by itself after this
                                  long.
                                </p>
                              </>
                            )}
                          </div>
                        )}
                        {linkAccess.can_manage &&
                          linkAccess.enabled &&
                          linkAccess.expires_at &&
                          linkExpiring && (
                            <form
                              className="lifetime-fields"
                              onSubmit={(e) => {
                                e.preventDefault();
                                const minutes = durationMinutes(linkExpiring);
                                if (minutes)
                                  void changeLink(selected, {
                                    enabled: true,
                                    minutes,
                                  });
                              }}
                            >
                              <label htmlFor="link-expiring-change">
                                End the link this long from now
                              </label>
                              <DurationPicker
                                id="link-expiring-change"
                                value={linkExpiring}
                                disabled={working}
                                onChange={setLinkExpiring}
                              />
                              <div className="lifetime-change-actions">
                                <Button
                                  type="submit"
                                  variant="outline"
                                  disabled={
                                    working ||
                                    durationMinutes(linkExpiring) === null
                                  }
                                >
                                  Set time
                                </Button>
                                <Button
                                  type="button"
                                  variant="ghost"
                                  disabled={working}
                                  onClick={() => setLinkExpiring(null)}
                                >
                                  Cancel
                                </Button>
                              </div>
                            </form>
                          )}
                      </div>
                      {linkAccess.can_manage && (
                        <div className="record-actions">
                          {linkAccess.enabled && linkAccess.expires_at && (
                            <>
                              <Button
                                variant="outline"
                                disabled={working}
                                onClick={() =>
                                  setLinkExpiring(DEFAULT_DURATION)
                                }
                              >
                                Change time
                              </Button>
                              <Button
                                variant="outline"
                                disabled={working}
                                onClick={() =>
                                  void changeLink(selected, { enabled: true })
                                }
                              >
                                Make permanent
                              </Button>
                            </>
                          )}
                          <Button
                            variant="outline"
                            disabled={
                              working ||
                              (!linkAccess.enabled &&
                                !!linkExpiring &&
                                durationMinutes(linkExpiring) === null)
                            }
                            onClick={() =>
                              void changeLink(
                                selected,
                                linkAccess.enabled
                                  ? { enabled: false }
                                  : {
                                      enabled: true,
                                      minutes: linkExpiring
                                        ? (durationMinutes(linkExpiring) ??
                                          undefined)
                                        : undefined,
                                    },
                              )
                            }
                          >
                            {linkAccess.enabled
                              ? 'Disable link sharing'
                              : linkExpiring
                                ? 'Enable expiring link'
                                : 'Enable link sharing'}
                          </Button>
                        </div>
                      )}
                    </div>
                  )}

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
                      <Plus size={15} /> Invite member, email, or tag
                    </Button>
                  )}
                  {grants.map((grant) => (
                    <div className="detail-record" key={grant.id}>
                      <div>
                        <strong>
                          {grant.principal.type}:{grant.principal.id}
                        </strong>
                        <p>{grant.access.join(', ')}</p>
                        {grant.expires_at && (
                          <Lifetime
                            kind="expiring"
                            at={grant.expires_at}
                            onElapsed={() =>
                              setGrants((value) =>
                                value.filter((g) => g.id !== grant.id),
                              )
                            }
                          />
                        )}
                        {grantChange?.id === grant.id && (
                          <form
                            className="lifetime-fields"
                            onSubmit={(e) => {
                              e.preventDefault();
                              const minutes = durationMinutes(
                                grantChange.duration,
                              );
                              if (minutes)
                                void changeExpiring(selected, grant, {
                                  minutes,
                                });
                            }}
                          >
                            <label htmlFor={'expiring-' + grant.id}>
                              End this share this long from now
                            </label>
                            <DurationPicker
                              id={'expiring-' + grant.id}
                              value={grantChange.duration}
                              disabled={working}
                              onChange={(duration) =>
                                setGrantChange({ id: grant.id, duration })
                              }
                            />
                            <div className="lifetime-change-actions">
                              <Button
                                type="submit"
                                variant="outline"
                                disabled={
                                  working ||
                                  durationMinutes(grantChange.duration) === null
                                }
                              >
                                Set time
                              </Button>
                              <Button
                                type="button"
                                variant="ghost"
                                disabled={working}
                                onClick={() => setGrantChange(null)}
                              >
                                Cancel
                              </Button>
                            </div>
                          </form>
                        )}
                      </div>
                      {selected.effective_access.includes(
                        'manage_permissions',
                      ) && (
                        <div className="record-actions">
                          {grant.expires_at && (
                            <>
                              <Button
                                variant="outline"
                                disabled={working}
                                onClick={() =>
                                  setGrantChange({
                                    id: grant.id,
                                    duration: DEFAULT_DURATION,
                                  })
                                }
                              >
                                Change time
                              </Button>
                              <Button
                                variant="outline"
                                disabled={working}
                                onClick={() =>
                                  void changeExpiring(selected, grant, {
                                    permanent: true,
                                  })
                                }
                              >
                                Make permanent
                              </Button>
                            </>
                          )}
                          <Button
                            variant="outline"
                            disabled={working}
                            onClick={async () => {
                              setDetailError('');
                              try {
                                await api(
                                  '/entries/' +
                                    selected.id +
                                    '/invitations/' +
                                    grant.id,
                                  'DELETE',
                                  { operation_id: crypto.randomUUID() },
                                );
                                setGrants((v) =>
                                  v.filter((g) => g.id !== grant.id),
                                );
                              } catch (e) {
                                detailFail(e);
                              }
                            }}
                          >
                            Revoke
                          </Button>
                        </div>
                      )}
                    </div>
                  ))}
                  {grantCursor && (
                    <Button
                      variant="outline"
                      onClick={async () => {
                        try {
                          const page = await api<{
                            items: Grant[];
                            next_cursor: string | null;
                          }>(
                            '/entries/' +
                              selected.id +
                              '/invitations?cursor=' +
                              encodeURIComponent(grantCursor),
                          );
                          setGrants((value) => [...value, ...page.items]);
                          setGrantCursor(page.next_cursor);
                        } catch (error) {
                          fail(error);
                        }
                      }}
                    >
                      More invitations
                    </Button>
                  )}
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
                          {item.actor_type}:{item.actor_id}
                          {item.app_id ? ' · ' + item.app_id : ''}
                        </p>
                        <p>{new Date(item.occurred_at).toLocaleString()}</p>
                      </div>
                    </div>
                  ))}
                  {logCursor && (
                    <Button
                      variant="outline"
                      disabled={working}
                      onClick={async () => {
                        setWorking(true);
                        try {
                          const page = await api<{
                            items: Activity[];
                            next_cursor: string | null;
                          }>(
                            '/entries/' +
                              selected.id +
                              '/logs?cursor=' +
                              encodeURIComponent(logCursor),
                          );
                          setActivity((v) => [...v, ...page.items]);
                          setLogCursor(page.next_cursor);
                        } catch (e) {
                          fail(e);
                        } finally {
                          setWorking(false);
                        }
                      }}
                    >
                      Older logs
                    </Button>
                  )}
                  {!activity.length && !detailLoading && (
                    <p>No recorded activity.</p>
                  )}
                </TabsContent>
              </Tabs>
            </div>
          )}
        </SheetContent>
      </Sheet>
    </div>
  );
}
