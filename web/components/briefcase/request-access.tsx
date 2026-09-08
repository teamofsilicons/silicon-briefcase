'use client';
import { useRef, useState, type SubmitEvent } from 'react';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Input } from '@/components/ui/input';
import { api } from '@/lib/api';

export default function RequestAccess({
  path,
  onError,
}: {
  path: string;
  onError: (error: unknown) => void;
}) {
  const [expanded, setExpanded] = useState(false),
    [reason, setReason] = useState('');
  const [rights, setRights] = useState(['read']),
    [busy, setBusy] = useState(false),
    [sent, setSent] = useState(false);
  const intent = useRef<{
    path: string;
    reason: string;
    access: string[];
    operation_id: string;
  } | null>(null);
  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const previous = intent.current;
    if (
      !previous ||
      previous.path !== path ||
      previous.reason !== reason ||
      previous.access.join() !== rights.join()
    ) {
      intent.current = {
        path,
        reason,
        access: rights,
        operation_id: crypto.randomUUID(),
      };
    }
    setBusy(true);
    try {
      await api('/access-requests', 'POST', intent.current);
      setSent(true);
    } catch (error) {
      onError(error);
    } finally {
      setBusy(false);
    }
  }
  if (sent)
    return (
      <output className="notice">
        Request sent. You’ll receive a notification when it is decided.
      </output>
    );
  return (
    <section className="request-access">
      <h2>File not found</h2>
      <p>If you were given this link, you can ask for access.</p>
      {!expanded ? (
        <Button variant="outline" onClick={() => setExpanded(true)}>
          Request access
        </Button>
      ) : (
        <form onSubmit={submit}>
          <fieldset disabled={busy}>
            <legend>Access to request</legend>
            <div className="rights">
              {['read', 'write', 'update', 'delete'].map((right) => (
                <label key={right}>
                  <Checkbox
                    checked={rights.includes(right)}
                    disabled={right === 'read'}
                    onCheckedChange={(checked) =>
                      setRights((value) =>
                        checked
                          ? [...value, right]
                          : value.filter((r) => r !== right),
                      )
                    }
                  />
                  {right === 'write' ? 'Create new content' : right}
                </label>
              ))}
            </div>
            <label htmlFor="access-reason">Message (optional)</label>
            <Input
              id="access-reason"
              maxLength={1024}
              value={reason}
              onChange={(event) => setReason(event.target.value)}
            />
            <Button type="submit">{busy ? 'Sending…' : 'Send request'}</Button>
          </fieldset>
        </form>
      )}
    </section>
  );
}
