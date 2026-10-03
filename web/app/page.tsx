'use client';
import { TelemetryPreference } from '@/components/briefcase/telemetry';
import { useCallback, useEffect, useRef, useState } from 'react';
import {
  ArrowRight,
  BriefcaseBusiness,
  Folder,
  KeyRound,
  LockKeyhole,
  ShieldCheck,
  Users,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import Workspace from '@/components/briefcase/workspace';
import TestSignIn from '@/components/briefcase/test-sign-in';
import PublicEntryView from '@/components/briefcase/public-entry';
import IamOrganizationsLink from '@/components/briefcase/iam-organizations-link';
import {
  api,
  setWorkspaceOrganization,
  setAccountContext,
  browserContextGeneration,
  testingEnvironment,
  returnToProduction,
  type AccountSession,
  type BrowserSession,
} from '@/lib/api';
import { readFileLocation } from '@/lib/file-location';
import {
  completeIamPopup,
  openIamPopup,
  continueIamInThisTab,
  safeLoginReturn,
  type IdentityKind,
} from '@/lib/iam-popup';
export default function Home() {
  const loginController = useRef<AbortController | null>(null);
  const loginAttempt = useRef<{
    nonce: string;
    started: Promise<unknown>;
  } | null>(null);
  const cancelAttempt = useCallback(async () => {
    loginController.current?.abort();
    const attempt = loginAttempt.current;
    loginAttempt.current = null;
    if (attempt) {
      // The durable nonce tombstone also cancels a start that has not arrived.
      // Late responses have separate cookies and cannot erase a successor.
      await api('/login/cancel', 'POST', { attempt_nonce: attempt.nonce });
    }
  }, []);
  useEffect(
    () => () => {
      void cancelAttempt().catch(() => {});
    },
    [cancelAttempt],
  );
  const [fullPagePending, setFullPagePending] = useState(false);
  const [busy, setBusy] = useState(false),
    [error, setError] = useState('');
  const [session, setSession] = useState<
      BrowserSession | AccountSession | null
    >(null),
    [checking, setChecking] = useState(true);
  const [publicDismissed, setPublicDismissed] = useState(false);
  const openPrivate = useCallback(() => setPublicDismissed(true), []);
  const [returnTo, setReturnTo] = useState('/');
  const [choosingOrganization, setChoosingOrganization] = useState(false);
  const signOut = useCallback(() => {
    try {
      const target = readFileLocation();
      setReturnTo(target ? location.pathname : '/');
    } catch {
      setReturnTo('/');
    }
    setSession(null);
    setChoosingOrganization(false);
    setWorkspaceOrganization(null);
    setAccountContext(null);
    window.location.reload();
  }, []);
  useEffect(() => {
    if (completeIamPopup()) return;
    const completion = new URLSearchParams(location.search);
    if (completion.has('iam_activate')) {
      const state = completion.get('iam_activate'),
        context = completion.get('context');
      history.replaceState(null, '', location.pathname);
      if (
        !/^[a-f0-9]{64}$/.test(state || '') ||
        !/^[a-f0-9-]{36}$/.test(context || '')
      ) {
        // eslint-disable-next-line react/react-compiler -- Validate callback parameters once after hydration.
        setError('Invalid sign-in completion. Start sign-in again.');
        setChecking(false);
        return;
      }
      sessionStorage.setItem(
        'briefcase-login-return',
        JSON.stringify({ state, context_id: context }),
      );
    }
    const retained = sessionStorage.getItem('briefcase-login-return');
    if (retained) {
      let active = true;
      void (async () => {
        try {
          const receipt = JSON.parse(retained);
          const current = await api<BrowserSession & { return_to: string }>(
            '/login/activate',
            'POST',
            receipt,
          );
          if (!active) return;
          if (
            !current.authenticated ||
            current.context_id !== receipt.context_id ||
            current.actor.type !==
              sessionStorage.getItem('briefcase-login-kind')
          )
            throw new Error(
              'The selected account could not be verified. Start sign-in again.',
            );
          const destination = safeLoginReturn(current.return_to);
          setAccountContext(current.context_id);
          sessionStorage.removeItem('briefcase-login-return');
          sessionStorage.removeItem('briefcase-login-kind');
          window.location.assign(destination);
        } catch (error) {
          if (!active) return;
          setError(
            error instanceof Error
              ? error.message
              : 'Sign-in could not be completed.',
          );
          setChecking(false);
        }
      })();
      return () => {
        active = false;
      };
    }
    const selectors = new URLSearchParams(location.search).getAll(
      'test_environment',
    );
    if (selectors.length) {
      if (
        selectors.length !== 1 ||
        !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
          selectors[0],
        ) ||
        selectors[0] === '00000000-0000-0000-0000-000000000000'
      ) {
        // eslint-disable-next-line react/react-compiler -- Validate the actual browser URL after static hydration before any session request.
        setError('Invalid testing environment in this link.');
        setChecking(false);
        return;
      }
      sessionStorage.setItem('briefcase-test-environment', selectors[0]);
    }
    try {
      const target = readFileLocation();
      if (target) {
        // eslint-disable-next-line react/react-compiler -- Hydrate the organisation from the actual browser URL, which is unavailable during static export.
        setReturnTo(location.pathname);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : 'File not found.');
    }
    if (new URLSearchParams(location.search).has('signin_error')) {
      setError('IAM sign-in could not be completed. Please start again.');
      history.replaceState(null, '', location.pathname);
    }
    api<BrowserSession | AccountSession>('/session')
      .then(async (value) => {
        setAccountContext(value.authenticated ? value.context_id : null);
        const target = readFileLocation();
        // A deep link selects a workspace only after IAM has supplied the
        // user's grants. It never contributes consent or scopes login.
        if (value.authenticated && target && value.org !== target.org) {
          if (
            value.contexts?.filter((context) => context.org === target.org)
              .length === 1
          ) {
            value = await api<BrowserSession>('/session', 'PATCH', {
              context_id: value.contexts.find(
                (context) => context.org === target.org,
              )?.context_id,
            });
          } else {
            setChoosingOrganization(true);
            setError(
              'Sign in to this organization with IAM, then open the file.',
            );
          }
        }
        setAccountContext(value.authenticated ? value.context_id : null);
        setWorkspaceOrganization(value.authenticated ? value.org : null);
        setSession(value.authenticated ? value : null);
      })
      .catch((e) => {
        if (e.status === 401) {
          setAccountContext(null);
          setWorkspaceOrganization(null);
          setSession(null);
        } else setError(e.message);
      })
      .finally(() => setChecking(false));
  }, []);
  async function login(kind: IdentityKind, fullPage = false) {
    if (testingEnvironment()) {
      returnToProduction();
      return;
    }
    if (loginController.current && !fullPage) return;
    const previous = cancelAttempt();
    void previous.catch(() => {});
    const controller = new AbortController();
    loginController.current = controller;
    const generation = browserContextGeneration();
    setBusy(true);
    setError('');
    setFullPagePending(fullPage);
    try {
      let boundAttempt = '';
      const start = async (nonce?: string) => {
        await previous;
        if (controller.signal.aborted) throw new Error('Sign-in cancelled.');
        boundAttempt =
          nonce ||
          Array.from(crypto.getRandomValues(new Uint8Array(32)), (value) =>
            value.toString(16).padStart(2, '0'),
          ).join('');
        sessionStorage.removeItem('briefcase-login-return');
        sessionStorage.setItem('briefcase-login-kind', kind);
        const started = api<{ redirect_url: string }>('/login/start', 'POST', {
          return_to: returnTo,
          identity_kind: kind,
          attempt_nonce: boundAttempt,
          ...(nonce ? { popup_nonce: nonce } : {}),
        });
        loginAttempt.current = { nonce: boundAttempt, started };
        const value = await started;
        if (
          controller.signal.aborted ||
          browserContextGeneration() !== generation
        )
          throw new Error('The workspace changed. Please start sign-in again.');
        return value.redirect_url;
      };
      if (fullPage) {
        await continueIamInThisTab(() => start(), controller.signal);
        if (loginController.current === controller) loginAttempt.current = null; // Full-page handoff owns the bounded server receipt.
        return;
      }
      const completed = await openIamPopup(start, controller.signal);
      if (
        controller.signal.aborted ||
        browserContextGeneration() !== generation
      )
        return;
      // Only the initiating tab adopts the candidate. Its public selector
      // stays authoritative when another tab's cookies arrive late.
      const current = await api<BrowserSession>('/login/activate', 'POST', {
        context_id: completed,
        attempt_nonce: boundAttempt,
      });
      if (
        !current.authenticated ||
        current.actor.type !== kind ||
        current.context_id !== completed
      )
        throw new Error(
          'The selected account could not be verified. Please sign in again.',
        );
      if (controller.signal.aborted) return;
      loginAttempt.current = null;
      setAccountContext(current.context_id);
      setWorkspaceOrganization(current.org);
      const destination = session
        ? '/org/' + encodeURIComponent(current.org || '') + '/'
        : returnTo;
      window.location.assign(destination);
    } catch (e) {
      const cancelled = controller.signal.aborted;
      if (loginController.current === controller)
        void cancelAttempt().catch(() => {});
      if (!cancelled) {
        setError(e instanceof Error ? e.message : 'Unable to reach Briefcase.');
      }
    } finally {
      if (loginController.current === controller)
        loginController.current = null;
      if (!loginController.current) {
        setBusy(false);
        setFullPagePending(false);
      }
    }
  }
  async function selectOrganization(selected: string) {
    setBusy(true);
    setError('');
    try {
      const next = await api<BrowserSession>('/session', 'PATCH', {
        context_id: selected,
      });
      setAccountContext(next.context_id);
      setWorkspaceOrganization(next.org);
      history.replaceState(
        null,
        '',
        '/org/' + encodeURIComponent(next.org) + '/',
      );
      setSession(next);
      setChoosingOrganization(false);
    } catch (e) {
      setError(
        e instanceof Error ? e.message : 'Unable to choose that workspace.',
      );
    } finally {
      setBusy(false);
    }
  }
  if (
    !checking &&
    !publicDismissed &&
    returnTo.startsWith('/org/') &&
    readFileLocation(returnTo)?.path
  )
    return <PublicEntryView authenticated={!!session} onSignIn={openPrivate} />;
  if (checking)
    return (
      <main className="loading-page">
        <BriefcaseBusiness size={32} />
        <output>Opening Briefcase…</output>
      </main>
    );
  if (session?.org != null && !choosingOrganization)
    return (
      <Workspace
        key={session.context_id}
        session={{ ...session, org: session.org }}
        onSignOut={signOut}
        onChooseOrganization={() => setChoosingOrganization(true)}
      />
    );
  return (
    <div className="entry-screen">
      {testingEnvironment() && (
        <aside className="testing-view-banner">
          <span>
            Your testing session needs a new sign-in. Return to production and
            open the environment again.
          </span>
          <Button onClick={returnToProduction}>Return to production</Button>
        </aside>
      )}
      <header className="masthead">
        {/* Full-page navigation resets a deep-link sign-in attempt. */}
        {/* eslint-disable-next-line next/no-html-link-for-pages */}
        <a className="brand" href="/">
          <BriefcaseBusiness aria-hidden="true" />
          <strong>briefcase</strong>
        </a>
        <a
          className="entry-docs"
          href="https://docs.briefcase.teamofsilicons.com/"
        >
          Help &amp; guides <ArrowRight size={14} aria-hidden="true" />
        </a>
      </header>
      <main className="signin-grid">
        <section className="signin-intro">
          <div className="eyebrow">
            {session ? 'Welcome back' : 'Your files, thoughtfully organised'}
          </div>
          <h1>
            A place for
            <br />
            your work<span className="blue">.</span>
          </h1>
          <p>
            Keep your files close and your team in the loop. A calm workspace
            for everything you’re working on.
          </p>
          <dl className="principles">
            <div>
              <dt>
                <Folder size={22} aria-hidden="true" />
                Public files
              </dt>
              <dd>Open to your organisation.</dd>
            </div>
            <div>
              <dt>
                <LockKeyhole size={22} aria-hidden="true" />
                Private files
              </dt>
              <dd>Private files. Your permissions.</dd>
            </div>
            <div>
              <dt>
                <Users size={22} aria-hidden="true" />
                Team spaces
              </dt>
              <dd>Shared spaces for your teams.</dd>
            </div>
          </dl>
        </section>
        <section className="signin-panel" aria-labelledby="signin-title">
          <div className="panel-kicker">
            <KeyRound size={15} aria-hidden="true" /> Your workspace awaits
          </div>
          <h2 id="signin-title">
            {session ? 'Where shall we work?' : 'Make yourself at home.'}
          </h2>
          {session ? (
            <>
              <p>Signed in as {session.actor.public_id}.</p>
              <p>Each account has its own organization and files.</p>
              <div className="organization-list">
                {(session.contexts ?? []).map((context) => (
                  <Button
                    className="organization-choice"
                    variant="outline"
                    key={context.context_id}
                    disabled={busy}
                    onClick={() => selectOrganization(context.context_id)}
                  >
                    <span className="organization-initial" aria-hidden="true">
                      {context.org.slice(0, 1).toUpperCase()}
                    </span>
                    <span>
                      {context.actor.public_id} · {context.org}
                    </span>
                    <ArrowRight size={16} aria-hidden="true" />
                  </Button>
                ))}
              </div>
              {session.organizations.length === 0 && (
                <output className="notice">
                  Organisation access needs reauthorisation. Continue with IAM
                  and choose the organisations Briefcase may access.
                </output>
              )}
              <IamOrganizationsLink />
              <p className="session-note">
                Add another account or organization
              </p>
              <div className="space-y-3">
                <Button
                  className="secondary-action"
                  variant="outline"
                  disabled={busy}
                  onClick={() => void login('carbon')}
                >
                  Continue as Carbon <ArrowRight size={18} />
                </Button>
                <Button
                  className="secondary-action"
                  variant="outline"
                  disabled={busy}
                  onClick={() => void login('silicon')}
                >
                  Continue as Silicon <ArrowRight size={18} />
                </Button>
              </div>
              {error && (
                <p className="error-box" role="alert">
                  {error}
                </p>
              )}
              <Button
                className="quiet-action"
                variant="ghost"
                disabled={busy}
                onClick={async () => {
                  setBusy(true);
                  setError('');
                  try {
                    await api('/session', 'DELETE');
                    signOut();
                  } catch (e) {
                    setError(
                      e instanceof Error ? e.message : 'Unable to sign out.',
                    );
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                Sign out
              </Button>
            </>
          ) : (
            <>
              <p>
                Sign in with your Silicon account to open your files and shared
                spaces. We’ll bring you right back here.
              </p>
              <div className="space-y-3">
                {error && (
                  <p className="error-box" role="alert">
                    {error}
                  </p>
                )}
                <Button
                  className="primary-action"
                  disabled={busy}
                  onClick={() => void login('carbon')}
                >
                  {busy ? 'Opening IAM…' : 'Continue as Carbon'}{' '}
                  <ArrowRight size={18} />
                </Button>
                <Button
                  className="secondary-action"
                  variant="outline"
                  disabled={busy}
                  onClick={() => void login('silicon')}
                >
                  {busy ? 'Opening IAM…' : 'Continue as Silicon'}{' '}
                  <ArrowRight size={18} />
                </Button>
              </div>
            </>
          )}
          <div className="session-note">
            {busy && (
              <Button
                type="button"
                variant="ghost"
                onClick={() => {
                  void cancelAttempt().catch((error) =>
                    setError(error.message),
                  );
                  loginController.current = null;
                  setBusy(false);
                  setFullPagePending(false);
                }}
              >
                Cancel sign-in
              </Button>
            )}
            <p>Or sign in in this tab</p>
            <Button
              type="button"
              variant="ghost"
              disabled={fullPagePending}
              onClick={() => void login('carbon', true)}
            >
              Continue as Carbon in this tab
            </Button>
            <Button
              type="button"
              variant="ghost"
              disabled={fullPagePending}
              onClick={() => void login('silicon', true)}
            >
              Continue as Silicon in this tab
            </Button>
          </div>
          {!testingEnvironment() && <TestSignIn />}
          <TelemetryPreference />
          <div className="session-note identity-note">
            <ShieldCheck size={15} aria-hidden="true" />
            <span>
              Your identity stays with Silicon IAM. You control who can access
              your files.
            </span>
          </div>
        </section>
      </main>
      <footer className="entry-footer">
        <span>A little space. A lot of possibility.</span>
        <a href="https://docs.briefcase.teamofsilicons.com/">
          Made for Carbons &amp; Silicons
        </a>
      </footer>
    </div>
  );
}
