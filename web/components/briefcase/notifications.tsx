'use client';
import { useCallback, useEffect, useState } from 'react';
import { Bell, RefreshCw } from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
  SheetDescription,
} from '@/components/ui/sheet';
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
  const [working, setWorking] = useState(false);
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
              </article>
            ))}
          </div>
        </SheetContent>
      </Sheet>
    </>
  );
}
