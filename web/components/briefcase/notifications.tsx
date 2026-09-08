'use client';
import { useCallback, useEffect, useState } from 'react';
import { Bell, RefreshCw } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
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
import { api, ApiError, type Entry } from '@/lib/api';

type Notice = {
  id: string;
  kind:
    | 'access_granted'
    | 'access_revoked'
    | 'access_requested'
    | 'access_request_decided';
  read: boolean;
  actor: { type: string; id: string } | null;
  subject: { entry_id: string; name: string; path: string } | null;
  access: string[] | null;
  access_request_id: string | null;
  decision: 'approved' | 'denied' | null;
  created_at: string;
};
type Inbox = { items: Notice[]; unread_count: number };
const labels = {
  access_granted: 'Access granted',
  access_revoked: 'Access removed',
  access_requested: 'Access requested',
  access_request_decided: 'Request decided',
};

export default function Notifications({
  onEntry,
  onUnauthorized,
}: {
  onEntry: (entry: Entry) => void;
  onUnauthorized: () => void;
}) {
  const [inbox, setInbox] = useState<Inbox>({ items: [], unread_count: 0 });
  const [open, setOpen] = useState(false),
    [loading, setLoading] = useState(false),
    [error, setError] = useState('');
  const [decision, setDecision] = useState<{
    notice: Notice;
    approve: boolean;
  } | null>(null);
  const [rights, setRights] = useState<string[]>([]),
    [working, setWorking] = useState(false);
  const [settled, setSettled] = useState<string[]>([]);
  const report = useCallback(
    (error: unknown) => {
      if (error instanceof ApiError && error.status === 401) {
        onUnauthorized();
        return;
      }
      setError(
        error instanceof Error
          ? error.message
          : 'The inbox could not be loaded.',
      );
    },
    [onUnauthorized],
  );
  const refresh = useCallback(async () => {
    setLoading(true);
    setError('');
    try {
      setInbox(await api<Inbox>('/notifications'));
    } catch (error) {
      report(error);
    } finally {
      setLoading(false);
    }
  }, [report]);
  useEffect(() => {
    // eslint-disable-next-line react/react-compiler -- Initialize the external inbox request and synchronize it again on window focus.
    void refresh();
    const focus = () => {
      void refresh();
    };
    window.addEventListener('focus', focus);
    return () => window.removeEventListener('focus', focus);
  }, [refresh]);
  async function decide() {
    if (!decision?.notice.access_request_id) return;
    setWorking(true);
    setError('');
    try {
      await api(
        '/access-requests/' + decision.notice.access_request_id + '/decision',
        'POST',
        decision.approve
          ? { decision: 'approve', access: rights }
          : { decision: 'deny' },
      );
      setSettled((values) => [...values, decision.notice.access_request_id!]);
      setDecision(null);
      await refresh();
    } catch (error) {
      report(error);
    } finally {
      setWorking(false);
    }
  }
  return (
    <>
      <Button
        variant="ghost"
        className="notification-trigger"
        aria-label={`Notifications, ${inbox.unread_count} unread`}
        onClick={() => {
          setOpen(true);
          void refresh();
        }}
      >
        <Bell size={19} />
        {inbox.unread_count > 0 && <span>{inbox.unread_count}</span>}
      </Button>
      <Sheet open={open} onOpenChange={setOpen}>
        <SheetContent className="details-sheet sm:max-w-xl">
          <SheetHeader>
            <SheetTitle>Notifications</SheetTitle>
            <SheetDescription>
              Your latest 20 notifications · {inbox.unread_count} unread
            </SheetDescription>
          </SheetHeader>
          <div className="detail-body">
            <div className="detail-actions">
              <Button
                variant="outline"
                disabled={working || !inbox.unread_count}
                onClick={async () => {
                  setWorking(true);
                  try {
                    setInbox(await api<Inbox>('/notifications/read', 'POST'));
                  } catch (error) {
                    report(error);
                  } finally {
                    setWorking(false);
                  }
                }}
              >
                Mark all as read
              </Button>
              <Button
                variant="ghost"
                disabled={loading}
                onClick={() => void refresh()}
              >
                <RefreshCw size={15} /> Refresh
              </Button>
            </div>
            {error && (
              <p role="alert" className="error-box">
                {error}
              </p>
            )}
            {loading && <output>Loading notifications…</output>}
            {!loading && !error && !inbox.items.length && (
              <p className="detail-hint">No notifications yet.</p>
            )}
            {inbox.items.map((notice) => (
              <article
                key={notice.id}
                className={
                  'notification-record' + (notice.read ? '' : ' unread')
                }
              >
                <h3>
                  {labels[notice.kind]}
                  {notice.decision ? ': ' + notice.decision : ''}
                </h3>
                {notice.subject && (
                  <Button
                    className="notification-subject"
                    variant="link"
                    onClick={async () => {
                      try {
                        const entry = await api<Entry>(
                          '/entries/' + notice.subject!.entry_id,
                        );
                        onEntry(entry);
                        setOpen(false);
                      } catch (error) {
                        report(error);
                      }
                    }}
                  >
                    {notice.subject.name}
                  </Button>
                )}
                {notice.actor && (
                  <p>
                    {notice.actor.type}:{notice.actor.id}
                  </p>
                )}
                {notice.access?.length ? (
                  <p>{notice.access.join(', ')}</p>
                ) : null}
                <time dateTime={notice.created_at}>
                  {new Date(notice.created_at).toLocaleString()}
                </time>
                {notice.kind === 'access_requested' &&
                  notice.access_request_id &&
                  !settled.includes(notice.access_request_id) && (
                    <div className="detail-actions">
                      <Button
                        variant="outline"
                        disabled={working}
                        onClick={() => {
                          setRights(notice.access || ['read']);
                          setDecision({ notice, approve: true });
                          setError('');
                        }}
                      >
                        Review access
                      </Button>
                      <Button
                        variant="ghost"
                        disabled={working}
                        onClick={() => {
                          setDecision({ notice, approve: false });
                          setError('');
                        }}
                      >
                        Deny
                      </Button>
                    </div>
                  )}
                {notice.access_request_id &&
                  settled.includes(notice.access_request_id) && (
                    <output>Request decided.</output>
                  )}
              </article>
            ))}
          </div>
        </SheetContent>
      </Sheet>
      <Dialog
        open={!!decision}
        onOpenChange={(open) => {
          if (!open && !working) setDecision(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {decision?.approve ? 'Approve access' : 'Deny this request?'}
            </DialogTitle>
            <DialogDescription>
              {decision?.notice.actor?.id} ·{' '}
              {decision?.notice.subject?.name || 'Requested entry'}
            </DialogDescription>
          </DialogHeader>
          {decision?.approve && (
            <fieldset disabled={working}>
              <legend>Permissions to grant</legend>
              <div className="rights">
                {['read', 'write', 'update', 'delete'].map((right) => (
                  <label key={right}>
                    <Checkbox
                      checked={rights.includes(right)}
                      onCheckedChange={(checked) =>
                        setRights((values) =>
                          checked
                            ? [...values, right]
                            : values.filter((value) => value !== right),
                        )
                      }
                    />
                    {right === 'write' ? 'Create new content' : right}
                  </label>
                ))}
              </div>
            </fieldset>
          )}
          {error && (
            <p className="error-box" role="alert">
              {error}
            </p>
          )}
          <Button
            disabled={working || (!!decision?.approve && !rights.length)}
            onClick={() => void decide()}
          >
            {working
              ? 'Saving…'
              : decision?.approve
                ? 'Grant selected access'
                : 'Deny request'}
          </Button>
        </DialogContent>
      </Dialog>
    </>
  );
}
