'use client';
import { useRef, useState } from 'react';
import { FlaskConical } from 'lucide-react';
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
  api,
  ApiError,
  enterTestingEnvironment,
  type BrowserSession,
} from '@/lib/api';

export default function TestingEnvironments({
  session,
  onUnauthorized,
}: {
  session: BrowserSession;
  onUnauthorized: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [appSecret, setAppSecret] = useState('');
  const [slt, setSlt] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const operation = useRef('');
  if (session.testing) return null;
  async function enter() {
    operation.current ||= crypto.randomUUID();
    setBusy(true);
    setError('');
    try {
      const result = await api<BrowserSession>('/environments/enter', 'POST', {
        app_secret: appSecret,
        slt,
        org: session.org,
        operation_id: operation.current,
      });
      if (!result.test_environment)
        throw new Error(
          'The server did not select a test environment. Retry entering testing mode.',
        );
      setAppSecret('');
      setSlt('');
      enterTestingEnvironment(result.test_environment.id);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) onUnauthorized();
      setError(
        e instanceof Error ? e.message : 'Unable to enter testing mode.',
      );
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <Button
        variant="ghost"
        className="organization-settings-trigger"
        onClick={() => setOpen(true)}
      >
        <FlaskConical size={17} /> Test environments
      </Button>
      <Sheet
        open={open}
        onOpenChange={(value) => {
          if (busy) return;
          setOpen(value);
          if (!value) {
            setAppSecret('');
            setSlt('');
            setError('');
            operation.current = '';
          }
        }}
      >
        <SheetContent className="details-sheet data-[side=right]:sm:max-w-2xl">
          <SheetHeader>
            <SheetTitle>Test environments</SheetTitle>
            <SheetDescription>
              Create, clean, disable, or restore shared environments in
              Honeycomb. Enter an existing environment here to test Briefcase
              files and permissions.
            </SheetDescription>
          </SheetHeader>
          <div className="detail-body">
            <a
              className="underline"
              href="https://console.honeycomb.teamofsilicons.com"
              target="_blank"
              rel="noreferrer"
            >
              Manage environments in Honeycomb ↗
            </a>
            <form
              className="mt-6 space-y-4"
              onSubmit={(event) => {
                event.preventDefault();
                void enter();
              }}
            >
              <h3 className="environment-section-title">
                Enter an existing environment
              </h3>
              <label className="block" htmlFor="briefcase-test-app-secret">
                Briefcase test app secret
                <Input
                  id="briefcase-test-app-secret"
                  type="password"
                  autoComplete="off"
                  value={appSecret}
                  required
                  disabled={busy}
                  onChange={(event) => {
                    setAppSecret(event.target.value);
                    operation.current = '';
                  }}
                />
              </label>
              <label className="block" htmlFor="briefcase-test-slt">
                Test SLT or Carbon/Silicon ID
                <Input
                  id="briefcase-test-slt"
                  type="password"
                  autoComplete="off"
                  value={slt}
                  required
                  disabled={busy}
                  onChange={(event) => {
                    setSlt(event.target.value);
                    operation.current = '';
                  }}
                />
              </label>
              <p className="text-sm text-muted-foreground">
                IAM authenticates the test identity. File actions follow that
                identity’s permissions.
              </p>
              {error && <p role="alert">{error}</p>}
              <Button type="submit" disabled={busy}>
                {busy ? 'Entering…' : 'Enter testing mode'}
              </Button>
            </form>
          </div>
        </SheetContent>
      </Sheet>
    </>
  );
}
