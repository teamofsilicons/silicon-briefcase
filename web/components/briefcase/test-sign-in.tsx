'use client';
import { useRef, useState, type SubmitEvent } from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { api, enterTestingEnvironment, type BrowserSession } from '@/lib/api';

export default function TestSignIn() {
  const [expanded, setExpanded] = useState(false);
  const [secret, setSecret] = useState('');
  const [slt, setSlt] = useState('');
  const [org, setOrg] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const operation = useRef('');
  function changed(set: (value: string) => void, value: string) {
    operation.current = '';
    set(value);
  }
  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!operation.current) operation.current = crypto.randomUUID();
    setBusy(true);
    setError('');
    try {
      const result = await api<BrowserSession>('/environments/enter', 'POST', {
        app_secret: secret,
        slt,
        ...(org.trim() ? { org: org.trim() } : {}),
        operation_id: operation.current,
      });
      if (!result.test_environment)
        throw new Error('Briefcase did not return a testing environment.');
      setSecret('');
      setSlt('');
      enterTestingEnvironment(result.test_environment.id);
    } catch (error) {
      setError(
        error instanceof Error
          ? error.message
          : 'Unable to sign in to the test environment.',
      );
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="test-sign-in">
      <Button
        variant="outline"
        disabled={busy}
        aria-expanded={expanded}
        onClick={() => {
          setExpanded(!expanded);
          setSecret('');
          setSlt('');
          setError('');
          operation.current = '';
        }}
      >
        {expanded ? 'Close testing sign-in' : 'Sign in to a test environment'}
      </Button>
      {expanded && (
        <form onSubmit={submit} className="test-sign-in-form">
          <p>
            Use your IAM test application secret and a test identity. A
            production login is optional.
          </p>
          <label htmlFor="test-org">Test organisation (optional)</label>
          <Input
            id="test-org"
            value={org}
            disabled={busy}
            autoComplete="off"
            onChange={(e) => changed(setOrg, e.target.value)}
            placeholder="tos"
          />
          <label htmlFor="test-app-secret">Test app secret</label>
          <Input
            id="test-app-secret"
            required
            type="password"
            value={secret}
            disabled={busy}
            autoComplete="off"
            onChange={(e) => changed(setSecret, e.target.value)}
          />
          <label htmlFor="test-slt">IAM test SLT or Carbon/Silicon ID</label>
          <Input
            id="test-slt"
            required
            type="password"
            value={slt}
            disabled={busy}
            autoComplete="off"
            onChange={(e) => changed(setSlt, e.target.value)}
          />
          {error && (
            <p className="error-box" role="alert">
              {error}
            </p>
          )}
          <Button type="submit" disabled={busy}>
            {busy ? 'Opening test environment…' : 'Enter test environment'}
          </Button>
        </form>
      )}
    </section>
  );
}
