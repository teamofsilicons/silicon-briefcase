'use client';

import { useEffect, useRef, useState } from 'react';
import {
  ArrowLeft,
  ChevronRight,
  Folder,
  LockKeyhole,
  LoaderCircle,
} from 'lucide-react';
import { api, type Entry, type Page } from '@/lib/api';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog';

/** Browsing and choosing are separate: read-only folders can lead to writable children. */
export default function FolderPicker({
  title,
  description,
  initialPath = '',
  excludePath,
  chooseLabel = 'Choose this folder',
  pendingLabel = 'Choosing…',
  onChoose,
  onClose,
}: {
  title: string;
  description: string;
  initialPath?: string;
  excludePath?: string;
  chooseLabel?: string;
  pendingLabel?: string;
  onChoose: (folder: Entry) => void | Promise<void>;
  onClose: () => void;
}) {
  const [path, setPath] = useState(initialPath);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [folder, setFolder] = useState<Entry | null>(null);
  const [cursor, setCursor] = useState<string | null>(null);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState('');
  const [choiceError, setChoiceError] = useState('');
  const [choosing, setChoosing] = useState(false);
  const choicePending = useRef(false);
  const mounted = useRef(true);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  useEffect(() => {
    let current = true;
    // eslint-disable-next-line react/react-compiler -- Start a cancellable external folder lookup when the requested destination changes.
    setBusy(true);
    setError('');
    const params = new URLSearchParams();
    if (path) params.set('path', path);
    if (nextCursor) params.set('cursor', nextCursor);
    void Promise.all([
      api<Page>('/entries?' + params),
      path
        ? api<Entry>('/resolve?path=' + encodeURIComponent(path))
        : Promise.resolve(null),
    ])
      .then(([page, parent]) => {
        if (!current) return;
        const folders = page.items.filter(
          (entry) =>
            entry.type === 'folder' &&
            (!excludePath ||
              (entry.path !== excludePath &&
                !entry.path.startsWith(excludePath + '/'))),
        );
        setEntries((previous) =>
          nextCursor ? [...previous, ...folders] : folders,
        );
        setFolder(parent);
        setCursor(page.next_cursor);
      })
      .catch((reason: unknown) => {
        if (current)
          setError(
            reason instanceof Error
              ? reason.message
              : 'Could not open this folder.',
          );
      })
      .finally(() => {
        if (current) setBusy(false);
      });
    return () => {
      current = false;
    };
  }, [path, nextCursor, excludePath]);
  function browse(value: string) {
    if (choicePending.current) return;
    setChoiceError('');
    setPath(value);
    setNextCursor(null);
    setEntries([]);
    setFolder(null);
  }
  const writable =
    !!folder?.effective_access.includes('write') && path !== 'private';
  function choose() {
    if (!folder || !writable || busy || error || choicePending.current) return;
    choicePending.current = true;
    setChoosing(true);
    setChoiceError('');
    const failed = (reason: unknown) => {
      if (mounted.current)
        setChoiceError(
          reason instanceof Error
            ? reason.message
            : 'Could not complete this action. Please try again.',
        );
    };
    const finished = () => {
      choicePending.current = false;
      if (mounted.current) setChoosing(false);
    };
    try {
      // Invoke before creating a Promise: the upload chooser must open the
      // native file dialog synchronously inside this user's click.
      const result = onChoose(folder);
      void Promise.resolve(result).catch(failed).finally(finished);
    } catch (reason) {
      failed(reason);
      finished();
    }
  }
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !choicePending.current) onClose();
      }}
    >
      <DialogContent className="destination-dialog" showCloseButton={!choosing}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>
        <div className="destination-path">
          <Button
            variant="ghost"
            size="icon"
            disabled={!path || busy || choosing}
            aria-label="Parent folder"
            onClick={() => browse(path.split('/').slice(0, -1).join('/'))}
          >
            <ArrowLeft size={17} />
          </Button>
          <span>{path || 'All spaces'}</span>
        </div>
        <div className="destination-folders" aria-busy={busy || choosing}>
          {entries.map((entry) => (
            <button
              key={entry.id}
              type="button"
              className="destination-folder"
              disabled={choosing}
              onClick={() => browse(entry.path)}
            >
              <Folder size={22} />
              <span>
                <strong>{entry.name}</strong>
                <small>
                  {entry.effective_access.includes('write')
                    ? 'Can add files'
                    : 'Browse folders'}
                </small>
              </span>
              <ChevronRight size={17} />
            </button>
          ))}
          {busy && (
            <output className="destination-empty">
              <LoaderCircle className="is-spinning" size={22} /> Opening
              folders…
            </output>
          )}
          {!busy && !entries.length && !error && (
            <p className="destination-empty">
              No folders inside this location.
            </p>
          )}
          {error && (
            <p className="error-box" role="alert">
              {error}
            </p>
          )}
          {cursor && (
            <Button
              variant="ghost"
              disabled={busy || choosing}
              onClick={() => setNextCursor(cursor)}
            >
              More folders
            </Button>
          )}
        </div>
        {!busy && !writable && (
          <p className="destination-help">
            <LockKeyhole size={14} /> Open a folder where you can add files.
          </p>
        )}
        {choiceError && (
          <p className="error-box" role="alert">
            {choiceError}
          </p>
        )}
        <div className="destination-actions">
          <Button variant="ghost" disabled={choosing} onClick={onClose}>
            Cancel
          </Button>
          <Button
            disabled={!writable || busy || !!error || choosing}
            onClick={choose}
          >
            {choosing && <LoaderCircle className="is-spinning" size={16} />}
            {choosing ? pendingLabel : chooseLabel}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
