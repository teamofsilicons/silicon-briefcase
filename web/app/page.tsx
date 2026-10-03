'use client';
import { TelemetryPreference } from '@/components/briefcase/telemetry';
import { useCallback, useEffect, useState } from 'react';
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
  testingEnvironment,
  returnToProduction,
  type AccountSession,
  type BrowserSession,
} from '@/lib/api';
import { readFileLocation } from '@/lib/file-location';
import {
  completeIamPopup,
  openIamPopup,
  type IdentityKind,
} from '@/lib/iam-popup';
export default function Home() {
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
        if (e.status !== 401) setError(e.message);
      })
      .finally(() => setChecking(false));
  }, []);
  async function login(kind: IdentityKind) {
    if (testingEnvironment()) {
      returnToProduction();
      return;
    }
    setBusy(true);
    setError('');
    try {
      await openIamPopup(async (nonce) => {
        const r = await fetch('/browser/login/start', {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            'X-Briefcase-Browser': '1',
          },
          body: JSON.stringify({
            return_to: returnTo,
            identity_kind: kind,
            popup_nonce: nonce,
          }),
        });
        const value = (await r.json()) as {
          error?: { message?: string };
          redirect_url: string;
        };
        if (!r.ok)
          throw new Error(
            value.error?.message || 'Sign-in could not be completed.',
          );
        return value.redirect_url;
      });
      // The popup changed the shared cookie. Fence old requests before reading
      // the new server context, and do not leave the previous account visible.
      setAccountContext(null);
      setWorkspaceOrganization(null);
      setSession(null);
      const current = await api<BrowserSession | AccountSession>('/session');
      if (!current.authenticated || current.actor.type !== kind)
        throw new Error(
          'The selected account could not be verified. Please sign in again.',
        );
      setAccountContext(current.context_id);
      setWorkspaceOrganization(current.org);
      const destination = session
        ? '/org/' + encodeURIComponent(current.org || '') + '/'
        : returnTo;
      window.location.assign(destination);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Unable to reach Briefcase.');
    } finally {
      setBusy(false);
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
