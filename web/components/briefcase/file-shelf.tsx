'use client';

import { useState, type ComponentType } from 'react';
import {
  ArrowUpRight,
  Check,
  Download,
  File,
  FileArchive,
  FileAudio,
  FileCode2,
  FileImage,
  FileSpreadsheet,
  FileText,
  FileVideo2,
  Folder,
  FolderInput,
  History,
  MoreHorizontal,
  Pencil,
  Presentation,
  RotateCcw,
  Share2,
  Trash2,
} from 'lucide-react';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { browserUrl, bytes, date, type Entry } from '@/lib/api';
import { Lifetime } from './lifetime';

export type FileShelfAction =
  | 'download'
  | 'rename'
  | 'move'
  | 'share'
  | 'delete'
  | 'restore'
  | 'details';

export type FileShelfScope = 'files' | 'recent' | 'bin' | 'search';

export type FileShelfProps = {
  entries: Entry[];
  mode: 'grid' | 'list';
  scope: FileShelfScope;
  selectedId?: string | null;
  loading?: boolean;
  onOpen: (entry: Entry) => void;
  onSelect: (entry: Entry) => void;
  onAction: (action: FileShelfAction, entry: Entry) => void;
  onSelfDestruct: (entry: Entry) => void;
};

type FileAppearance = {
  icon: ComponentType<{ size?: number; strokeWidth?: number }>;
  label: string;
  tone: string;
};

const APPEARANCES: Record<string, FileAppearance> = {
  image: { icon: FileImage, label: 'Image', tone: 'image' },
  video: { icon: FileVideo2, label: 'Video', tone: 'video' },
  audio: { icon: FileAudio, label: 'Audio', tone: 'audio' },
  document: { icon: FileText, label: 'Document', tone: 'document' },
  spreadsheet: {
    icon: FileSpreadsheet,
    label: 'Spreadsheet',
    tone: 'spreadsheet',
  },
  presentation: {
    icon: Presentation,
    label: 'Presentation',
    tone: 'presentation',
  },
  archive: { icon: FileArchive, label: 'Archive', tone: 'archive' },
  code: { icon: FileCode2, label: 'Code', tone: 'code' },
};

// The API normally supplies its canonical render category. Older responses and
// imported metadata can omit it; use the same extension-first fallback as the
// backend's media classifier without fetching any file contents.
const EXTENSION_KINDS = Object.fromEntries(
  Object.entries({
    image:
      'apng avif bmp gif heic heif ico jfif jpeg jpg pjpeg png svg svgz tif tiff webp',
    video: '3g2 3gp avi flv m4v mkv mov mp4 mpeg mpg ogv webm wmv',
    document: 'doc docx epub md markdown odt pages pdf rst rtf tex txt',
    spreadsheet: 'csv numbers ods tsv xls xlsb xlsm xlsx',
    presentation: 'key odp pot potx ppt pptx',
    audio: 'aac aiff amr flac m4a mid midi mp3 oga ogg opus wav wma',
    archive: '7z br bz2 cab dmg gz gzip iso jar rar tar tgz txz xz zip zst',
    code: 'bat c cc cfg clj conf cpp cs css dart diff ejs elm env erl ex exs go gradle graphql h hbs hpp hs htm html ini ipynb java js json jsonl jsx kt kts less lock log lua m mjs ml patch php pl properties proto ps1 py r rb rs sass scala scss sh sql svelte swift tf toml ts tsx vue xml yaml yml zsh',
  }).flatMap(([kind, endings]) =>
    endings.split(' ').map((ending) => [ending, kind]),
  ),
);

const MIME_KINDS: [string, string[]][] = [
  ['image', ['image/']],
  ['video', ['video/']],
  ['audio', ['audio/']],
  [
    'document',
    [
      'text/plain',
      'text/markdown',
      'text/richtext',
      'application/pdf',
      'application/msword',
      'application/vnd.openxmlformats-officedocument.wordprocessing',
      'application/vnd.oasis.opendocument.text',
    ],
  ],
  [
    'spreadsheet',
    [
      'text/csv',
      'text/tab-separated-values',
      'application/vnd.ms-excel',
      'application/vnd.openxmlformats-officedocument.spreadsheet',
      'application/vnd.oasis.opendocument.spreadsheet',
    ],
  ],
  [
    'presentation',
    [
      'application/vnd.ms-powerpoint',
      'application/vnd.openxmlformats-officedocument.presentation',
      'application/vnd.oasis.opendocument.presentation',
    ],
  ],
  [
    'archive',
    [
      'application/zip',
      'application/gzip',
      'application/x-tar',
      'application/x-7z-compressed',
      'application/x-bzip2',
      'application/x-rar',
      'application/vnd.rar',
    ],
  ],
  [
    'code',
    [
      'text/',
      'application/json',
      'application/xml',
      'application/javascript',
      'application/sql',
      'application/yaml',
      'application/x-yaml',
    ],
  ],
];

function fileKind(entry: Entry): string {
  const rendered = entry.render?.trim().toLowerCase() || '';
  if (Object.hasOwn(APPEARANCES, rendered)) return rendered;
  if (rendered === 'markdown') return 'document';
  const ending = entry.name.includes('.')
    ? entry.name.split('.').at(-1)?.toLowerCase()
    : '';
  if (ending && Object.hasOwn(EXTENSION_KINDS, ending))
    return EXTENSION_KINDS[ending];
  const mime = entry.content_type?.trim().toLowerCase() || '';
  return (
    MIME_KINDS.find(([, prefixes]) =>
      prefixes.some((prefix) => mime.startsWith(prefix)),
    )?.[0] || 'file'
  );
}

function appearance(entry: Entry): FileAppearance {
  if (entry.type === 'folder')
    return { icon: Folder, label: 'Folder', tone: 'folder' };
  return (
    APPEARANCES[fileKind(entry)] || {
      icon: File,
      label: 'File',
      tone: 'file',
    }
  );
}

function extension(entry: Entry): string {
  const ending = entry.name.includes('.') ? entry.name.split('.').at(-1) : '';
  return ending && /^[a-z0-9]{1,8}$/i.test(ending)
    ? ending.toUpperCase()
    : appearance(entry).label;
}

/** Only small, readable image originals are used as thumbnails. */
function thumbnailUrl(entry: Entry, enabled: boolean): string | null {
  if (
    !enabled ||
    entry.type !== 'file' ||
    fileKind(entry) !== 'image' ||
    !entry.effective_access.includes('read') ||
    entry.deleted_at ||
    entry.size == null ||
    entry.size <= 0 ||
    entry.size > 5 * 1024 * 1024
  )
    return null;
  return browserUrl(
    '/browser/entries/' + encodeURIComponent(entry.id) + '/content',
  );
}

function FileArtwork({
  entry,
  thumbnail,
}: {
  entry: Entry;
  thumbnail: boolean;
}) {
  const [failedImage, setFailedImage] = useState<string | null>(null);
  const source = thumbnailUrl(entry, thumbnail);
  const { icon: Icon, tone } = appearance(entry);

  if (source && source !== failedImage)
    return (
      <span className="bc-shelf__image-frame" aria-hidden="true">
        {/* Authenticated originals must stay on the browser's cookie-bound origin. */}
        {/* eslint-disable-next-line next/no-img-element */}
        <img
          alt=""
          src={source}
          loading="lazy"
          decoding="async"
          draggable={false}
          referrerPolicy="no-referrer"
          onError={() => setFailedImage(source)}
        />
      </span>
    );

  if (entry.type === 'folder')
    return (
      <span className="bc-shelf__folder-art" aria-hidden="true">
        <Folder size={78} strokeWidth={1.2} />
      </span>
    );

  return (
    <span className="bc-shelf__paper" data-tone={tone} aria-hidden="true">
      <span className="bc-shelf__paper-format">{extension(entry)}</span>
      <span className="bc-shelf__paper-icon">
        <Icon size={44} strokeWidth={1.35} />
      </span>
      <span className="bc-shelf__paper-name">
        {entry.name.replace(/\.[^.]+$/, '') || entry.name}
      </span>
    </span>
  );
}

type EntryActionsProps = Pick<
  FileShelfProps,
  'onOpen' | 'onAction' | 'onSelect'
> & {
  entry: Entry;
  inBin: boolean;
  selected: boolean;
};

function EntryMenu({
  entry,
  inBin,
  selected,
  onOpen,
  onSelect,
  onAction,
}: EntryActionsProps) {
  const canRead = entry.effective_access.includes('read');
  const canUpdate = entry.effective_access.includes('update');
  const canShare = entry.effective_access.includes('manage_permissions');
  const canDelete = entry.effective_access.includes('delete');

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <button
            type="button"
            className="bc-shelf__icon-button"
            aria-label={'Actions for ' + entry.name}
            title={'Actions for ' + entry.name}
          />
        }
      >
        <MoreHorizontal size={18} aria-hidden="true" />
      </DropdownMenuTrigger>
      <DropdownMenuContent
        className="bc-shelf__menu"
        align="end"
        sideOffset={8}
      >
        {inBin ? (
          <DropdownMenuItem onClick={() => onAction('restore', entry)}>
            <RotateCcw size={16} /> Restore
          </DropdownMenuItem>
        ) : (
          <>
            {(canRead || entry.type === 'folder') && (
              <DropdownMenuItem onClick={() => onOpen(entry)}>
                <ArrowUpRight size={16} />
                {entry.type === 'folder' ? 'Open folder' : 'Preview'}
              </DropdownMenuItem>
            )}
            {!selected && entry.type === 'folder' && (
              <DropdownMenuItem onClick={() => onSelect(entry)}>
                <Check size={16} /> Select folder
              </DropdownMenuItem>
            )}
            {canRead && (
              <DropdownMenuItem onClick={() => onAction('download', entry)}>
                <Download size={16} /> Download
              </DropdownMenuItem>
            )}
            {canShare && (
              <DropdownMenuItem onClick={() => onAction('share', entry)}>
                <Share2 size={16} /> Share
              </DropdownMenuItem>
            )}
            {canUpdate && (
              <>
                <DropdownMenuItem onClick={() => onAction('rename', entry)}>
                  <Pencil size={16} /> Rename
                </DropdownMenuItem>
                <DropdownMenuItem onClick={() => onAction('move', entry)}>
                  <FolderInput size={16} /> Move
                </DropdownMenuItem>
              </>
            )}
            <DropdownMenuItem onClick={() => onAction('details', entry)}>
              <History size={16} /> Details & history
            </DropdownMenuItem>
            {canDelete && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  variant="destructive"
                  onClick={() => onAction('delete', entry)}
                >
                  <Trash2 size={16} />
                  {entry.self_destruct_at
                    ? 'Delete permanently'
                    : 'Move to bin'}
                </DropdownMenuItem>
              </>
            )}
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function ActionDock(props: EntryActionsProps) {
  const { entry, inBin, onOpen, onAction } = props;
  const canRead = entry.effective_access.includes('read');
  return (
    <fieldset className="bc-shelf__dock" aria-label={'Selected ' + entry.name}>
      {inBin ? (
        <button
          type="button"
          className="bc-shelf__dock-primary"
          onClick={() => onAction('restore', entry)}
        >
          <RotateCcw size={16} aria-hidden="true" /> Restore
        </button>
      ) : (
        <>
          {(canRead || entry.type === 'folder') && (
            <button
              type="button"
              className="bc-shelf__dock-primary"
              onClick={() => onOpen(entry)}
            >
              <ArrowUpRight size={16} aria-hidden="true" />
              {entry.type === 'folder' ? 'Open' : 'Preview'}
            </button>
          )}
          {entry.effective_access.includes('manage_permissions') && (
            <button
              type="button"
              className="bc-shelf__icon-button"
              aria-label={'Share ' + entry.name}
              title="Share"
              onClick={() => onAction('share', entry)}
            >
              <Share2 size={16} aria-hidden="true" />
            </button>
          )}
          {canRead && (
            <button
              type="button"
              className="bc-shelf__icon-button"
              aria-label={'Download ' + entry.name}
              title="Download"
              onClick={() => onAction('download', entry)}
            >
              <Download size={16} aria-hidden="true" />
            </button>
          )}
          <EntryMenu {...props} />
        </>
      )}
    </fieldset>
  );
}

export default function FileShelf({
  entries,
  mode,
  scope,
  selectedId,
  loading = false,
  onOpen,
  onSelect,
  onAction,
  onSelfDestruct,
}: FileShelfProps) {
  const inBin = scope === 'bin';
  const displayedEntries =
    mode === 'grid'
      ? [
          ...entries.filter((entry) => entry.type === 'file'),
          ...entries.filter((entry) => entry.type === 'folder'),
        ]
      : entries;

  return (
    <section
      className={'bc-shelf bc-shelf--' + mode}
      aria-label={inBin ? 'Deleted files and folders' : 'Files and folders'}
      aria-busy={loading}
    >
      {loading && entries.length === 0 && (
        <output className="bc-shelf__sr-only">Loading files…</output>
      )}
      <ul className="bc-shelf__items">
        {loading && entries.length === 0
          ? Array.from({ length: mode === 'grid' ? 4 : 5 }, (_, index) => (
              <li className="bc-shelf__skeleton" key={index} aria-hidden="true">
                <span />
                <i />
              </li>
            ))
          : displayedEntries.map((entry) => {
              const selected = selectedId === entry.id;
              const folder = entry.type === 'folder';
              const fileAppearance = appearance(entry);
              const actions = {
                entry,
                inBin,
                selected,
                onOpen,
                onSelect,
                onAction,
              };
              return (
                <li
                  key={entry.id}
                  className="bc-shelf__item"
                  data-selected={selected}
                  data-folder={folder}
                  data-bin={inBin}
                >
                  <button
                    type="button"
                    className="bc-shelf__main"
                    aria-label={
                      (folder && !inBin ? 'Open ' : 'Select ') + entry.name
                    }
                    aria-pressed={folder && !inBin ? undefined : selected}
                    onClick={() =>
                      folder && !inBin ? onOpen(entry) : onSelect(entry)
                    }
                  >
                    <span className="bc-shelf__well">
                      <FileArtwork
                        key={entry.id + ':' + entry.updated_at}
                        entry={entry}
                        thumbnail={mode === 'grid' && !inBin}
                      />
                      {selected && (
                        <span
                          className="bc-shelf__selected-mark"
                          aria-hidden="true"
                        >
                          <Check size={16} strokeWidth={2.7} />
                        </span>
                      )}
                    </span>
                    <span className="bc-shelf__caption">
                      <span className="bc-shelf__name" title={entry.name}>
                        {entry.name}
                      </span>
                      <span className="bc-shelf__summary">
                        {folder ? 'Folder' : extension(entry)}
                        {!folder && (
                          <>
                            <span aria-hidden="true"> · </span>
                            {bytes(entry.size)}
                          </>
                        )}
                        <span className="bc-shelf__summary-date">
                          <span aria-hidden="true"> · </span>
                          {inBin ? 'Deleted ' : ''}
                          {date(inBin ? entry.deleted_at : entry.updated_at)}
                        </span>
                      </span>
                      {(scope === 'recent' || scope === 'search' || inBin) && (
                        <span className="bc-shelf__path" title={entry.path}>
                          {entry.path}
                        </span>
                      )}
                      {entry.visibility === 'traversal' && !inBin && (
                        <span className="bc-shelf__access-note">
                          Shared contents only
                        </span>
                      )}
                    </span>
                    {mode === 'list' && (
                      <span className="bc-shelf__list-meta" aria-hidden="true">
                        <span>{fileAppearance.label}</span>
                        <span>
                          {date(inBin ? entry.deleted_at : entry.updated_at)}
                        </span>
                        <span>{folder ? '—' : bytes(entry.size)}</span>
                      </span>
                    )}
                  </button>
                  {!selected && (
                    <div className="bc-shelf__more">
                      <EntryMenu {...actions} />
                    </div>
                  )}
                  {entry.self_destruct_at && !inBin && (
                    <div className="bc-shelf__lifetime">
                      <Lifetime
                        key={entry.self_destruct_at}
                        kind="self-destruct"
                        at={entry.self_destruct_at}
                        onElapsed={() => onSelfDestruct(entry)}
                      />
                    </div>
                  )}
                  {selected && <ActionDock {...actions} />}
                </li>
              );
            })}
      </ul>
    </section>
  );
}
