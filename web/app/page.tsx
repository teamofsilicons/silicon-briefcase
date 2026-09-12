'use client';
import { useCallback, useEffect, useState, type SubmitEvent } from 'react';
import {
  ArrowRight,
  BriefcaseBusiness,
  KeyRound,
  ShieldCheck,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import Workspace from '@/components/briefcase/workspace';
import PublicEntryView from '@/components/briefcase/public-entry';
import IamOrganizationsLink from '@/components/briefcase/iam-organizations-link';
import {
  api,
  setWorkspaceOrganization,
  testingEnvironment,
  returnToProduction,
  type AccountSession,
  type BrowserSession,
} from '@/lib/api';
import { readFileLocation } from '@/lib/file-location';
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
  }, []);
  useEffect(() => {
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
        const target = readFileLocation();
        // A deep link selects a workspace only after IAM has supplied the
        // user's grants. It never contributes consent or scopes login.
        if (value.authenticated && target && value.org !== target.org) {
          if (value.organizations.includes(target.org)) {
            value = await api<BrowserSession>('/session', 'PATCH', {
              org: target.org,
            });
          } else {
            setChoosingOrganization(true);
            setError(
              'This workspace was not granted to Briefcase. Continue with IAM to review your organisation selection.',
            );
          }
        }
        setWorkspaceOrganization(value.authenticated ? value.org : null);
        setSession(value.authenticated ? value : null);
      })
      .catch((e) => {
        if (e.status !== 401) setError(e.message);
      })
      .finally(() => setChecking(false));
  }, []);
  async function login(event: SubmitEvent) {
    event.preventDefault();
    if (testingEnvironment()) {
      returnToProduction();
      return;
    }
    setBusy(true);
    setError('');
    try {
      const r = await fetch('/browser/login/start', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'X-Briefcase-Browser': '1',
        },
        body: JSON.stringify({ return_to: returnTo }),
      });
      const value = (await r.json()) as {
        error?: { message?: string };
        redirect_url: string;
      };
      if (!r.ok)
        throw new Error(
          value.error?.message || 'Sign-in could not be completed.',
        );
      window.location.assign(value.redirect_url);
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
        org: selected,
      });
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
  if (!checking && !publicDismissed && returnTo.startsWith('/org/'))
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
        key={session.org}
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
          {/* eslint-disable-next-line next/no-img-element -- Local shared Silicon brand asset. */}
          <img src="/brand/mark.svg" alt="" width={28} height={28} />
          <strong>silicon</strong>
          <span>BRIEFCASE</span>
        </a>
        <a href="https://github.com/teamofsilicons/silicon-briefcase/tree/main/docs">
          Documentation <ArrowRight size={15} />
        </a>
      </header>
      <main className="signin-grid">
        <section className="signin-intro">
          <div className="eyebrow">
            {session ? 'YOUR ACCOUNT' : 'YOUR FILES'}
          </div>
          <h1>
            Open your <br />
            Briefcase<span className="blue">.</span>
          </h1>
          <p>
            Sign in as yourself. Your files, shared folders, and organisation
            spaces will be waiting.
          </p>
          <dl className="principles">
            <div>
              <dt>01 / Public</dt>
              <dd>Shared across your organisation.</dd>
            </div>
            <div>
              <dt>02 / Private</dt>
              <dd>Your files, with access you control.</dd>
            </div>
            <div>
              <dt>03 / Tags</dt>
              <dd>Spaces for the teams you belong to.</dd>
            </div>
          </dl>
          <div className="identity-note">
            <ShieldCheck size={19} />
            <span>Identity by Silicon IAM. Permissions by Briefcase.</span>
          </div>
        </section>
        <section className="signin-panel" aria-labelledby="signin-title">
          <div className="panel-kicker">
            <KeyRound size={18} /> MEMBER ACCESS
          </div>
          <h2 id="signin-title">
            {session ? 'Your organisations' : 'Sign in with IAM'}
          </h2>
          {session ? (
            <>
              <p>Signed in as {session.actor.public_id}.</p>
              <p>Open a workspace you authorised in IAM.</p>
              <div className="organization-list">
                {session.organizations.map((organization) => (
                  <Button
                    className="primary-action"
                    key={organization}
                    disabled={busy}
                    onClick={() => selectOrganization(organization)}
                  >
                    {organization}
                    <ArrowRight size={18} />
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
              <form onSubmit={login}>
                <Button
                  className="primary-action"
                  disabled={busy}
                  type="submit"
                >
                  {busy ? 'Continuing…' : 'Review organisation access in IAM'}
                  <ArrowRight size={18} />
                </Button>
              </form>
              {error && (
                <p className="error-box" role="alert">
                  {error}
                </p>
              )}
              <Button
                className="primary-action"
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
                Continue to Silicon IAM to verify your identity. You’ll return
                here automatically.
              </p>
              <form onSubmit={login}>
                {error && (
                  <p className="error-box" role="alert">
                    {error}
                  </p>
                )}
                <Button
                  className="primary-action"
                  type="submit"
                  disabled={busy}
                >
                  {testingEnvironment()
                    ? 'Return to production to sign in'
                    : busy
                      ? 'Continuing to IAM…'
                      : 'Continue with IAM'}
                  <ArrowRight size={18} />
                </Button>
              </form>
            </>
          )}
          <p className="session-note">
            Your session stays on the server. Tokens aren’t saved in browser
            storage.
          </p>
        </section>
      </main>
      <footer className="entry-footer">
        <span>TEAM OF SILICONS</span>
        <span>Files for Carbons & Silicons</span>
      </footer>
    </div>
  );
}
