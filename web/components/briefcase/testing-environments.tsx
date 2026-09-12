'use client';
import { useRef, useState, type SubmitEvent } from 'react';
import { FlaskConical, Plus, RefreshCw, Copy, Eye, EyeOff } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
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
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  api,
  ApiError,
  enterTestingEnvironment,
  type BrowserSession,
} from '@/lib/api';

type Environment = {
  id: string;
  org_id: string;
  name: string;
  description: string | null;
  status: 'active' | 'deleted';
  iam_environment_id: string;
  iam_app_id: string;
  created_by: { type: string; id: string };
  key_generation: number;
  version: number;
  last_activity_at: string;
  cleaned_at: string | null;
  purge_after: string | null;
};
type Pairing = {
  iam_environment_id: string;
  iam_environment_key: string;
  iam_app_id: string;
  iam_app_secret: string;
};
type Kind =
  | 'create'
  | 'edit'
  | 'pairing'
  | 'key'
  | 'clean'
  | 'retire'
  | 'restore';
type Draft = {
  kind: Kind;
  environment?: Environment;
  name: string;
  description: string;
  pairing: Pairing;
  confirmation: string;
  operation_id: string;
};
type Secret = { environment: string; name: string; key: string };
type Result = Partial<Environment> & {
  key?: string;
  environment_id?: string;
  erased_rows?: number;
};
const labels: Record<Kind, string> = {
  create: 'Create environment',
  edit: 'Edit environment',
  pairing: 'Replace IAM pairing',
  key: 'Reveal app secret',
  clean: 'Clean environment',
  retire: 'Retire environment',
  restore: 'Restore environment',
};
const blankPairing = (): Pairing => ({
  iam_environment_id: '',
  iam_environment_key: '',
  iam_app_id: '',
  iam_app_secret: '',
});
const destructive = (kind: Kind) =>
  ['clean', 'retire', 'pairing'].includes(kind);

export default function TestingEnvironments({
  session,
  onUnauthorized,
}: {
  session: BrowserSession;
  onUnauthorized: () => void;
}) {
  const [open, setOpen] = useState(false),
    [filter, setFilter] = useState<'active' | 'deleted'>('active');
  const [items, setItems] = useState<Environment[]>([]),
    [loading, setLoading] = useState(false),
    [listError, setListError] = useState('');
  const [draft, setDraft] = useState<Draft | null>(null),
    [busy, setBusy] = useState(false),
    [uncertain, setUncertain] = useState(false),
    [error, setError] = useState(''),
    [notice, setNotice] = useState('');
  const [secret, setSecret] = useState<Secret | null>(null),
    [showSecret, setShowSecret] = useState(false),
    [copied, setCopied] = useState(false);
  const [directSecret, setDirectSecret] = useState(''),
    [directSlt, setDirectSlt] = useState('');
  const directOperation = useRef('');
  const generation = useRef(0),
    intent = useRef<{ path: string; method: string; body?: unknown } | null>(
      null,
    );

  const pendingOperation = useRef<string | null>(null);
  const [viewEnvironment, setViewEnvironment] = useState<Environment | null>(
    null,
  );
  const [viewToken, setViewToken] = useState('');
  const [viewError, setViewError] = useState('');
  const [viewBusy, setViewBusy] = useState(false);
  const viewOperation = useRef('');
  async function enterSecret() {
    if (!directOperation.current) directOperation.current = crypto.randomUUID();
    setBusy(true);
    setListError('');
    try {
      const value = await api<BrowserSession>('/environments/enter', 'POST', {
        app_secret: directSecret,
        slt: directSlt,
        org: session.org,
        operation_id: directOperation.current,
      });
      setDirectSecret('');
      setDirectSlt('');
      if (value.test_environment)
        enterTestingEnvironment(value.test_environment.id);
    } catch (e) {
      setListError(
        e instanceof Error ? e.message : 'Unable to enter testing mode.',
      );
    } finally {
      setBusy(false);
    }
  }
  async function openView(environment: Environment, slt?: string) {
    setViewBusy(true);
    setViewError('');
    try {
      await api('/environments/' + environment.id + '/view', 'POST', {
        operation_id: viewOperation.current,
        ...(slt ? { slt } : {}),
      });
      setViewToken('');
      enterTestingEnvironment(environment.id);
    } catch (e) {
      setViewError(
        e instanceof ApiError && e.status === 401
          ? 'Sign in with a fresh token from the paired IAM testing environment.'
          : e instanceof Error
            ? e.message
            : 'The environment could not be opened.',
      );
    } finally {
      setViewBusy(false);
    }
  }
  function beginView(environment: Environment) {
    setViewEnvironment(environment);
    setViewToken('');
    setViewError('');
    viewOperation.current = crypto.randomUUID();
    void openView(environment);
  }

  async function load(status: 'active' | 'deleted' = filter) {
    const ticket = ++generation.current;
    setLoading(true);
    setListError('');
    try {
      const page = await api<{ items: Environment[] }>(
        '/environments?status=' + status,
      );
      if (ticket === generation.current) setItems(page.items);
    } catch (error) {
      if (ticket === generation.current) {
        if (error instanceof ApiError && error.status === 401) onUnauthorized();
        else
          setListError(
            error instanceof Error
              ? error.message
              : 'Environments could not be loaded.',
          );
      }
    } finally {
      if (ticket === generation.current) setLoading(false);
    }
  }
  function begin(kind: Kind, environment?: Environment) {
    setError('');
    setUncertain(false);
    setDraft({
      kind,
      environment,
      name: environment?.name || '',
      description: environment?.description || '',
      pairing: {
        ...blankPairing(),
        iam_environment_id: environment?.iam_environment_id || '',
        iam_app_id: environment?.iam_app_id || '',
      },
      confirmation: '',
      operation_id: crypto.randomUUID(),
    });
  }
  function dismiss() {
    if (busy) return;
    setDraft(null);
    intent.current = null;
    setUncertain(false);
    setError('');
  }
  function changePairing(key: keyof Pairing, value: string) {
    setDraft((previous) =>
      previous
        ? { ...previous, pairing: { ...previous.pairing, [key]: value } }
        : null,
    );
  }
  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!draft) return;
    if (
      destructive(draft.kind) &&
      draft.confirmation !== draft.environment?.name
    ) {
      setError('Type the exact environment name to confirm.');
      return;
    }
    if (pendingOperation.current !== draft.operation_id) {
      intent.current = null;
      pendingOperation.current = draft.operation_id;
    }
    if (!intent.current) {
      const base =
        '/environments' + (draft.environment ? '/' + draft.environment.id : '');
      if (draft.kind === 'create')
        intent.current = {
          path: base,
          method: 'POST',
          body: {
            name: draft.name,
            description: draft.description || null,
            iam_test_key: draft.pairing.iam_environment_key || undefined,
            operation_id: draft.operation_id,
          },
        };
      else if (draft.kind === 'edit')
        intent.current = {
          path: base,
          method: 'PATCH',
          body: {
            name: draft.name,
            description: draft.description || null,
            version: draft.environment!.version,
            operation_id: draft.operation_id,
          },
        };
      else if (draft.kind === 'pairing')
        intent.current = {
          path: base + '/pairing',
          method: 'POST',
          body: {
            pairing: { ...draft.pairing },
            operation_id: draft.operation_id,
          },
        };
      else
        intent.current = {
          path: base + '/' + draft.kind,
          method: 'POST',
          body:
            draft.kind === 'key'
              ? undefined
              : { operation_id: draft.operation_id },
        };
    }
    const request = intent.current;
    setBusy(true);
    setError('');
    try {
      const result = await api<Result>(
        request.path,
        request.method,
        request.body,
      );
      if (['create', 'restore', 'key'].includes(draft.kind)) {
        const id = result.environment_id || result.id || draft.environment?.id;
        if (typeof result.key !== 'string' || !id)
          throw new Error(
            'The app-secret response was incomplete. Recover the same operation.',
          );
        setSecret({
          environment: id,
          name: result.name || draft.environment?.name || draft.name,
          key: result.key,
        });
        setShowSecret(false);
        setCopied(false);
      }
      setNotice(
        draft.kind === 'clean'
          ? `Cleaned ${draft.environment!.name}: ${result.erased_rows ?? 0} records erased. Stored objects are queued for deletion.`
          : draft.kind === 'retire'
            ? `${draft.environment!.name} retired. Its app secret is invalid; recovery is available for two days.`
            : draft.kind === 'key'
              ? ''
              : labels[draft.kind] + ' completed.',
      );
      setDraft(null);
      intent.current = null;
      setUncertain(false);
      await load();
    } catch (error) {
      const rejected =
        error instanceof ApiError &&
        [400, 403, 404, 405, 413, 415, 422].includes(error.status);
      setUncertain(!rejected);
      if (rejected) intent.current = null;
      if (error instanceof ApiError && error.status === 401) onUnauthorized();
      else
        setError(
          error instanceof Error
            ? error.message
            : 'The result could not be confirmed.',
        );
    } finally {
      setBusy(false);
    }
  }
  if (session.testing) return null;
  const locked = busy || uncertain;
  return (
    <>
      <Button
        variant="ghost"
        className="organization-settings-trigger"
        onClick={() => {
          setOpen(true);
          void load();
        }}
      >
        <FlaskConical size={17} /> Test environments
      </Button>
      <Sheet
        open={open}
        onOpenChange={(value) => {
          if (!busy) setOpen(value);
        }}
      >
        <SheetContent className="details-sheet sm:max-w-3xl">
          <SheetHeader>
            <SheetTitle>Test environments</SheetTitle>
            <SheetDescription>
              {session.org} · Management using your production identity. These
              actions target only the selected test environment.
            </SheetDescription>
          </SheetHeader>
          <form
            className="settings-form"
            onSubmit={(e) => {
              e.preventDefault();
              void enterSecret();
            }}
          >
            <h3>Enter with an IAM test app secret</h3>
            <label htmlFor="direct-app-secret">
              App secret
              <Input
                id="direct-app-secret"
                type="password"
                autoComplete="off"
                required
                pattern="ask_[A-Za-z0-9_-]{43}"
                value={directSecret}
                onChange={(e) => {
                  setDirectSecret(e.target.value);
                  directOperation.current = '';
                }}
                placeholder="ask_…"
              />
            </label>
            <label htmlFor="direct-test-slt">
              IAM test sign-in token
              <Input
                id="direct-test-slt"
                type="password"
                autoComplete="off"
                required
                value={directSlt}
                onChange={(e) => {
                  setDirectSlt(e.target.value);
                  directOperation.current = '';
                }}
              />
            </label>
            <p>
              The secret selects the environment. Your test IAM account
              determines file permissions. Limit: 2 GiB.
            </p>
            <Button type="submit" disabled={busy}>
              Enter testing mode
            </Button>
          </form>

          <div className="detail-body">
            <div className="environment-toolbar">
              <Select
                value={filter}
                onValueChange={(value) => {
                  if (value === 'active' || value === 'deleted') {
                    setFilter(value);
                    setItems([]);
                    void load(value);
                  }
                }}
              >
                <SelectTrigger aria-label="Environment status">
                  <SelectValue>
                    {filter === 'active' ? 'Active' : 'Retired / recoverable'}
                  </SelectValue>
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="active">Active</SelectItem>
                  <SelectItem value="deleted">Retired / recoverable</SelectItem>
                </SelectContent>
              </Select>
              <Button
                variant="outline"
                disabled={loading}
                onClick={() => void load()}
              >
                <RefreshCw size={15} /> Refresh
              </Button>
              <Button onClick={() => begin('create')}>
                <Plus size={15} /> Create
              </Button>
            </div>
            <p className="detail-hint">
              Each environment has its own paired IAM test world and up to 2 GiB
              of storage. Inactivity for one day retires it; retired
              environments have a two-day recovery window. Briefcase supports up
              to 10 active environments across the deployment.
            </p>
            <p className="detail-hint">
              Open an active environment to browse its files as a test Carbon or
              Silicon. Your production workspace remains available.
            </p>
            {loading && <output>Loading environments…</output>}
            {listError && (
              <p role="alert" className="error-box">
                {listError}
              </p>
            )}
            {notice && <output className="notice">{notice}</output>}
            {!loading && !listError && !items.length && (
              <p className="detail-hint">
                No {filter === 'active' ? 'active' : 'recoverable retired'}{' '}
                environments.
              </p>
            )}
            {items.map((environment) => (
              <article className="environment-record" key={environment.id}>
                <h3>{environment.name}</h3>
                <p>{environment.description}</p>
                <dl>
                  <div>
                    <dt>Environment ID</dt>
                    <dd>
                      <code>{environment.id}</code>
                    </dd>
                  </div>
                  <div>
                    <dt>IAM environment</dt>
                    <dd>
                      <code>{environment.iam_environment_id}</code>
                    </dd>
                  </div>
                  <div>
                    <dt>IAM application</dt>
                    <dd>{environment.iam_app_id}</dd>
                  </div>
                  <div>
                    <dt>Created by</dt>
                    <dd>
                      {environment.created_by.type}:{environment.created_by.id}
                    </dd>
                  </div>
                  <div>
                    <dt>Key generation</dt>
                    <dd>{environment.key_generation}</dd>
                  </div>
                  <div>
                    <dt>Last activity</dt>
                    <dd>
                      {new Date(environment.last_activity_at).toLocaleString()}
                    </dd>
                  </div>
                  {environment.purge_after && (
                    <div>
                      <dt>Recovery deadline</dt>
                      <dd>
                        {new Date(environment.purge_after).toLocaleString()}
                      </dd>
                    </div>
                  )}
                </dl>
                <div className="environment-actions">
                  {environment.status === 'active' && (
                    <Button
                      onClick={() => beginView(environment)}
                      disabled={viewBusy}
                    >
                      <FlaskConical size={16} />
                      View as testing environment
                    </Button>
                  )}
                  {environment.status === 'deleted' ? (
                    <Button
                      variant="outline"
                      onClick={() => begin('restore', environment)}
                    >
                      Restore
                    </Button>
                  ) : (
                    (
                      ['edit', 'key', 'pairing', 'clean', 'retire'] as Kind[]
                    ).map((kind) => (
                      <Button
                        key={kind}
                        variant={
                          kind === 'clean' || kind === 'retire'
                            ? 'destructive'
                            : 'outline'
                        }
                        onClick={() => begin(kind, environment)}
                      >
                        {labels[kind]}
                      </Button>
                    ))
                  )}
                </div>
              </article>
            ))}
          </div>
        </SheetContent>
      </Sheet>
      <Dialog
        open={!!viewEnvironment}
        onOpenChange={(value) => {
          if (!value && !viewBusy) {
            setViewEnvironment(null);
            setViewToken('');
            setViewError('');
          }
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>View as testing environment</DialogTitle>
            <DialogDescription>{viewEnvironment?.name}</DialogDescription>
          </DialogHeader>
          <p>
            Browse files, previews, sharing, and uploads using this
            environment’s test account.
          </p>
          <form
            className="space-y-4"
            onSubmit={(event) => {
              event.preventDefault();
              if (viewEnvironment)
                void openView(viewEnvironment, viewToken.trim());
            }}
          >
            {viewError && (
              <p role="alert" className="error-box">
                {viewError}
              </p>
            )}
            <label className="block" htmlFor="test-signin-token">
              IAM test sign-in token
              <Input
                id="test-signin-token"
                aria-label="IAM test sign-in token"
                type="password"
                autoComplete="off"
                value={viewToken}
                onChange={(e) => {
                  setViewToken(e.target.value);
                  viewOperation.current = crypto.randomUUID();
                }}
                required
                disabled={viewBusy}
              />
            </label>
            <p className="detail-hint">
              Use a fresh token for{' '}
              <strong>{viewEnvironment?.iam_app_id}</strong> from IAM test
              environment <code>{viewEnvironment?.iam_environment_id}</code>.
              Production tokens cannot be used here.
            </p>
            <Button type="submit" disabled={viewBusy}>
              {viewBusy ? 'Opening…' : 'Enter testing environment'}
            </Button>
          </form>
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!draft}
        onOpenChange={(value) => {
          if (!value) dismiss();
        }}
      >
        <DialogContent className="environment-dialog sm:max-w-xl">
          <DialogHeader>
            <DialogTitle>{draft && labels[draft.kind]}</DialogTitle>
            <DialogDescription>
              {draft?.environment ? (
                <>
                  {draft.environment.name} · <code>{draft.environment.id}</code>
                </>
              ) : (
                'Create an empty Briefcase test environment paired with an existing IAM test environment.'
              )}
            </DialogDescription>
          </DialogHeader>
          <form onSubmit={submit}>
            <fieldset disabled={locked} className="environment-fields">
              {(draft?.kind === 'create' || draft?.kind === 'edit') && (
                <>
                  <label htmlFor="environment-name">
                    Name
                    <Input
                      id="environment-name"
                      required
                      maxLength={100}
                      value={draft.name}
                      onChange={(event) =>
                        setDraft((previous) =>
                          previous
                            ? { ...previous, name: event.target.value }
                            : null,
                        )
                      }
                    />
                  </label>
                  <label htmlFor="environment-description">
                    Description (optional)
                    <Input
                      id="environment-description"
                      maxLength={1000}
                      value={draft.description}
                      onChange={(event) =>
                        setDraft((previous) =>
                          previous
                            ? { ...previous, description: event.target.value }
                            : null,
                        )
                      }
                    />
                  </label>
                </>
              )}
              {draft?.kind === 'pairing' && (
                <>
                  <p className="detail-hint">
                    Use credentials from the same IAM test environment.
                    Application IDs are canonical, for example org&gt;briefcase.
                    These values are submitted to Briefcase for validation and
                    encrypted storage; they are not saved in browser storage.
                  </p>
                  <label htmlFor="pairing-id">
                    IAM test environment UUID
                    <Input
                      id="pairing-id"
                      required
                      value={draft.pairing.iam_environment_id}
                      maxLength={36}
                      onChange={(event) =>
                        changePairing('iam_environment_id', event.target.value)
                      }
                      autoComplete="off"
                    />
                  </label>
                  <label htmlFor="pairing-key">
                    IAM test app secret
                    <Input
                      id="pairing-key"
                      type="password"
                      required
                      pattern="[A-Za-z0-9]{32}"
                      minLength={32}
                      maxLength={32}
                      value={draft.pairing.iam_environment_key}
                      onChange={(event) =>
                        changePairing('iam_environment_key', event.target.value)
                      }
                      autoComplete="off"
                    />
                  </label>
                  <label htmlFor="pairing-app">
                    Test application ID
                    <Input
                      id="pairing-app"
                      required
                      maxLength={255}
                      value={draft.pairing.iam_app_id}
                      onChange={(event) =>
                        changePairing('iam_app_id', event.target.value)
                      }
                      autoComplete="off"
                    />
                  </label>
                  <label htmlFor="pairing-secret">
                    Test application secret
                    <Input
                      id="pairing-secret"
                      type="password"
                      required
                      maxLength={256}
                      value={draft.pairing.iam_app_secret}
                      onChange={(event) =>
                        changePairing('iam_app_secret', event.target.value)
                      }
                      autoComplete="off"
                    />
                  </label>
                </>
              )}
              {draft?.kind === 'clean' && (
                <p className="error-box">
                  This erases all disposable data in this environment and queues
                  its stored objects for deletion. Cleaning cannot be undone by
                  restoring the environment. Its configuration and app secret
                  remain.
                </p>
              )}
              {draft?.kind === 'retire' && (
                <p className="detail-hint">
                  This disables access to the environment. It can be restored
                  for two days using its current IAM app secret, provided the
                  IAM environment is still active.
                </p>
              )}
              {draft?.kind === 'restore' && (
                <p className="detail-hint">
                  Restore this environment before its purge deadline. The
                  current IAM app secret will be shown after success.
                </p>
              )}
              {draft?.kind === 'pairing' && (
                <p className="detail-hint">
                  This replaces the complete IAM pairing and the selected app
                  key. Existing IAM projection and cross-environment migration
                  safeguards still apply.
                </p>
              )}
              {draft?.kind === 'key' && (
                <p className="detail-hint">
                  This audited operation retrieves the current Briefcase root
                  key. It selects the test plane; test actor authentication is
                  still separate. Store it in a secret manager.
                </p>
              )}
              {draft && destructive(draft.kind) && (
                <label htmlFor="environment-confirm">
                  Type “{draft.environment!.name}” to confirm
                  <Input
                    id="environment-confirm"
                    required
                    value={draft.confirmation}
                    onChange={(event) =>
                      setDraft((previous) =>
                        previous
                          ? { ...previous, confirmation: event.target.value }
                          : null,
                      )
                    }
                    autoComplete="off"
                  />
                </label>
              )}
            </fieldset>
            {error && (
              <p role="alert" className="error-box">
                {error}
              </p>
            )}
            {uncertain && (
              <p className="detail-hint">
                The outcome is uncertain. Retry this exact operation. If you
                close this dialog, check the environment’s state before starting
                another operation.
              </p>
            )}
            {draft && draft.kind !== 'key' && (
              <p className="settings-operation">
                Operation ID: <code>{draft.operation_id}</code>
              </p>
            )}
            <div className="environment-actions">
              <Button
                type="button"
                variant="outline"
                disabled={busy}
                onClick={dismiss}
              >
                Close
              </Button>
              <Button
                type="submit"
                disabled={
                  busy ||
                  (!!draft &&
                    destructive(draft.kind) &&
                    draft.confirmation !== draft.environment?.name)
                }
                variant={
                  draft && destructive(draft.kind) ? 'destructive' : 'default'
                }
              >
                {busy
                  ? 'Working…'
                  : uncertain
                    ? 'Retry same operation'
                    : draft && labels[draft.kind]}
              </Button>
            </div>
          </form>
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!secret}
        onOpenChange={(value) => {
          if (!value) {
            setSecret(null);
            setShowSecret(false);
            setCopied(false);
            setError('');
          }
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Briefcase test app secret</DialogTitle>
            <DialogDescription>
              {secret?.name} · <code>{secret?.environment}</code>
            </DialogDescription>
          </DialogHeader>
          <p className="detail-hint">
            Save this in a secret manager. The value is held only in this dialog
            and disappears when it closes.
          </p>
          <Input
            aria-label="Test app secret"
            type={showSecret ? 'text' : 'password'}
            readOnly
            value={secret?.key || ''}
            autoComplete="off"
          />
          <div className="environment-actions">
            <Button
              variant="outline"
              onClick={() => setShowSecret((value) => !value)}
            >
              {showSecret ? <EyeOff size={16} /> : <Eye size={16} />}{' '}
              {showSecret ? 'Hide' : 'Show'}
            </Button>
            <Button
              variant="outline"
              onClick={async () => {
                if (!secret) return;
                try {
                  await navigator.clipboard.writeText(secret.key);
                  setCopied(true);
                } catch {
                  setError(
                    'Clipboard access failed. Show the key and copy it manually.',
                  );
                }
              }}
            >
              <Copy size={16} /> Copy key
            </Button>
          </div>
          {copied && (
            <output>Copied. Clear your clipboard after storing the key.</output>
          )}
          {error && (
            <p role="alert" className="error-box">
              {error}
            </p>
          )}
        </DialogContent>
      </Dialog>
    </>
  );
}
