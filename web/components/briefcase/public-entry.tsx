/* eslint-disable next/no-img-element -- Shared images must retain the authenticated origin and cannot use a remote image optimizer. */
/* eslint-disable jsx-a11y/media-has-caption -- User-uploaded media has no supplied captions; do not fabricate a caption track. */
'use client';
import { useCallback, useEffect, useState } from 'react';
import Link from 'next/link';
import { File, Folder, Download, ArrowRight } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { bytes } from '@/lib/api';
import { readTextPreview } from '@/lib/text-preview';
import { textFormat } from '@/lib/preview-document';
import { richPreview } from '@/lib/rich-preview';
import { fileLocation, readFileLocation } from '@/lib/file-location';

type PublicEntry = {
  id: string;
  name: string;
  path: string;
  entry_type: 'file' | 'folder';
  content_type: string | null;
  size: number | null;
};
type Page = { items: PublicEntry[]; next_cursor: string | null };

export default function PublicEntryView({
  onSignIn,
  authenticated = false,
}: {
  onSignIn: () => void;
  authenticated?: boolean;
}) {
  const [entry, setEntry] = useState<PublicEntry | null>(null),
    [children, setChildren] = useState<PublicEntry[]>([]);
  const [cursor, setCursor] = useState<string | null>(null),
    [error, setError] = useState(''),
    [loading, setLoading] = useState(true);
  const [org, setOrg] = useState('');
  const [preview, setPreview] = useState<{
    document: string;
    note: string;
    truncated: boolean;
  } | null>(null);
  const target = () => readFileLocation();
  function endpoint(path: string, view = 'metadata', cursor?: string) {
    const current = target();
    return (
      '/browser/public?' +
      new URLSearchParams({
        org: current?.org || '',
        path,
        view,
        ...(cursor ? { cursor } : {}),
      })
    );
  }
  const read = useCallback(
    async function read<T>(url: string, signal?: AbortSignal): Promise<T> {
      const response = await fetch(url, {
        credentials: 'omit',
        signal,
        cache: 'no-store',
        redirect: 'error',
      });
      if (!response.ok) {
        if (response.status === 404 && authenticated) {
          onSignIn();
          throw new Error('Opening workspace…');
        }
        throw new Error(
          response.status === 404
            ? 'File not found.'
            : 'This link could not be opened. Try again.',
        );
      }
      return response.json() as Promise<T>;
    },
    [authenticated, onSignIn],
  );
  useEffect(() => {
    const abort = new AbortController();
    const load = async () => {
      try {
        const current = target();
        if (!current?.path) throw new Error('File not found.');
        setOrg(current.org);
        const item = await read<PublicEntry>(
          endpoint(current.path),
          abort.signal,
        );
        setEntry(item);
        if (item.entry_type === 'folder') {
          const page = await read<Page>(
            endpoint(current.path, 'contents'),
            abort.signal,
          );
          setChildren(page.items);
          setCursor(page.next_cursor);
        } else {
          const format = textFormat({ ...item, render: null });
          if (format) {
            const excerpt = await readTextPreview(
              endpoint(item.path, 'inline'),
              abort.signal,
              item.size === 0,
              'omit',
            );
            const rendered = await richPreview(
              excerpt.text,
              format,
              abort.signal,
            );
            if (!abort.signal.aborted)
              setPreview({ ...rendered, truncated: excerpt.truncated });
          }
        }
      } catch (e) {
        if (!abort.signal.aborted)
          setError(e instanceof Error ? e.message : 'File not found.');
      } finally {
        if (!abort.signal.aborted) setLoading(false);
      }
    };
    void load();
    return () => abort.abort();
  }, [read]);
  const source = entry ? endpoint(entry.path, 'inline') : '';
  return (
    <main className="public-share">
      <header>
        <Link href="/">Silicon Briefcase</Link>
        <Button variant="outline" onClick={onSignIn}>
          {authenticated ? 'Open workspace' : 'Sign in'}{' '}
          <ArrowRight size={16} />
        </Button>
      </header>
      {loading ? (
        <output>Opening shared link…</output>
      ) : error ? (
        <div role="alert">
          <h1>{error}</h1>
          <p>Sign in if this entry was shared with your account.</p>
        </div>
      ) : (
        entry && (
          <>
            <p className="detail-hint">
              Shared from {org} · Anyone with this link can view and download
            </p>
            <h1>{entry.name}</h1>
            <p>
              {entry.entry_type === 'folder'
                ? 'Shared folder'
                : bytes(entry.size)}
            </p>
            <a
              className="download-link"
              href={endpoint(entry.path, 'attachment')}
            >
              <Download size={16} />
              Download{entry.entry_type === 'folder' ? ' .tar.zst' : ''}
            </a>
            {entry.entry_type === 'folder' ? (
              <div className="public-files">
                {children.map((child) => (
                  <a key={child.id} href={fileLocation(org, child.path)}>
                    {child.entry_type === 'folder' ? (
                      <Folder size={20} />
                    ) : (
                      <File size={20} />
                    )}
                    <span>{child.name}</span>
                    <small>{bytes(child.size)}</small>
                  </a>
                ))}
                {!children.length && <p>This folder is empty.</p>}
                {cursor && (
                  <Button
                    disabled={loading}
                    onClick={async () => {
                      setLoading(true);
                      try {
                        const page = await read<Page>(
                          endpoint(entry.path, 'contents', cursor),
                        );
                        setChildren((v) => [...v, ...page.items]);
                        setCursor(page.next_cursor);
                      } catch (e) {
                        setError(
                          e instanceof Error
                            ? e.message
                            : 'Unable to load more.',
                        );
                      } finally {
                        setLoading(false);
                      }
                    }}
                  >
                    More files
                  </Button>
                )}
              </div>
            ) : entry.content_type?.startsWith('image/') ? (
              <img className="public-preview" src={source} alt={entry.name} />
            ) : entry.content_type?.startsWith('video/') ? (
              <video className="public-preview" controls src={source} />
            ) : entry.content_type?.startsWith('audio/') ? (
              <audio controls src={source} />
            ) : preview ? (
              <div className="text-preview">
                {preview.truncated && (
                  <p>
                    Showing the first 1 MiB. Download for the complete contents.
                  </p>
                )}
                {preview.note && <p>{preview.note}</p>}
                <iframe
                  className="public-preview"
                  title={entry.name}
                  sandbox=""
                  referrerPolicy="no-referrer"
                  srcDoc={preview.document}
                />
              </div>
            ) : entry.content_type === 'application/pdf' ? (
              <iframe
                className="public-preview"
                title={entry.name}
                sandbox=""
                src={source}
              />
            ) : (
              <p>Preview unavailable for this format. Download to open it.</p>
            )}
          </>
        )
      )}
    </main>
  );
}
