#!/usr/bin/env node
/**
 * Local, disposable data for visual QA. This is never part of the production
 * gateway. Run explicitly with `node scripts/design-preview.mjs` and point the
 * Vite development proxy at http://127.0.0.1:4328.
 *
 * No network calls, credentials, or disk writes. State is isolated to this
 * process and resets on restart. `--signed-out` starts at the sign-in screen.
 * Login is a local simulation, not IAM authentication. S3 configuration,
 * testing credentials, and folder archives are intentionally unsupported.
 * Uploading `rejected.txt` simulates a 413 rejection. Uploading
 * `recover-on-retry.txt` drops the first response after saving; retry the same
 * operation_id to recover the committed entry without creating a duplicate.
 */
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { randomUUID, createHash } from 'node:crypto';

const ORG = 'design-preview';
const ACTOR = { type: 'carbon', public_id: 'c:preview' };
const ACCESS = ['read', 'write', 'update', 'delete', 'manage_permissions'];
const ASSETS = new URL('./fixtures/', import.meta.url);
const MAX_UPLOAD = 10 * 1024 * 1024;
const CAMPAIGN = 'Design/Autumn campaign';
const copy = `# A little more space for your best work\n\nAutumn campaign · Website copy\n\n## The idea\n\nGood work deserves room to breathe. Make space for the things that matter, and keep everything else beautifully simple.\n\n## Headline\n\nA considered place for every idea.\n\n## Supporting copy\n\nFrom the first sketch to the final handoff, bring your team's work together in one calm, connected place.\n\n## Call to action\n\nMake room for what's next.\n\n---\n\nLocal design-preview content. No customer files are included.\n`;

function previewPdf() {
  const stream = `0.97 0.96 0.93 rg 0 0 612 792 re f\n0.12 0.37 0.72 rg 54 540 504 8 re f\nBT /F1 12 Tf 0.35 0.35 0.35 rg 54 715 Td (AUTUMN CAMPAIGN / DESIGN PREVIEW) Tj ET\nBT /F1 48 Tf 0.13 0.15 0.19 rg 54 630 Td (Brand guide.) Tj ET\nBT /F1 18 Tf 0.35 0.35 0.35 rg 54 580 Td (A considered place for every idea.) Tj ET\nBT /F1 14 Tf 0.25 0.25 0.25 rg 54 480 Td (01  Make room for what matters.) Tj 0 -32 Td (02  Be clear, calm, and consistent.) Tj 0 -32 Td (03  Use blue with intention.) Tj 0 -80 Td (Accent: #1F5FB8) Tj 0 -32 Td (Light surfaces. Generous space. Useful details.) Tj ET\nBT /F1 10 Tf 0.5 0.5 0.5 rg 54 60 Td (Local fixture document. No customer content.) Tj ET`;
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>',
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
    `<< /Length ${Buffer.byteLength(stream)} >>\nstream\n${stream}\nendstream`,
  ];
  let output = '%PDF-1.4\n';
  const offsets = [0];
  objects.forEach((object, index) => {
    offsets.push(Buffer.byteLength(output));
    output += `${index + 1} 0 obj\n${object}\nendobj\n`;
  });
  const xref = Buffer.byteLength(output);
  output += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  output += offsets
    .slice(1)
    .map((offset) => `${String(offset).padStart(10, '0')} 00000 n \n`)
    .join('');
  output += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return Buffer.from(output);
}

function isLoopback(hostname) {
  return ['localhost', '127.0.0.1', '[::1]', '::1'].includes(hostname);
}

function error(status, message) {
  return Object.assign(new Error(message), { status });
}

// Mirrors src/domain/media.rs: a known extension outranks a declared MIME type.
// These are the wire enum values, not the text-preview worker's format names.
const RENDER_EXTENSIONS = {
  image:
    'apng avif bmp gif heic heif ico jfif jpeg jpg pjpeg png svg svgz tif tiff webp',
  video: '3g2 3gp avi flv m4v mkv mov mp4 mpeg mpg ogv webm wmv',
  document: 'doc docx epub md markdown odt pages pdf rst rtf tex txt',
  spreadsheet: 'csv numbers ods tsv xls xlsb xlsm xlsx',
  presentation: 'key odp pot potx ppt pptx',
  audio: 'aac aiff amr flac m4a mid midi mp3 oga ogg opus wav wma',
  archive: '7z br bz2 cab dmg gz gzip iso jar rar tar tgz txz xz zip zst',
  code: 'bat c cc cfg clj conf cpp cs css dart diff ejs elm env erl ex exs go gradle graphql h hbs hpp hs htm html ini ipynb java js json jsonl jsx kt kts less lock log lua m mjs ml patch php pl properties proto ps1 py r rb rs sass scala scss sh sql svelte swift tf toml ts tsx vue xml yaml yml zsh',
};

function renderKind(name, contentType) {
  const extension = name.includes('.')
    ? name.split('.').at(-1).toLowerCase()
    : '';
  for (const [kind, extensions] of Object.entries(RENDER_EXTENSIONS))
    if (extension && extensions.split(' ').includes(extension)) return kind;
  const [top, subtype] = (contentType || '')
    .split(';')[0]
    .trim()
    .toLowerCase()
    .split('/');
  if (['image', 'video', 'audio'].includes(top)) return top;
  if (top === 'text') {
    if (['csv', 'tab-separated-values'].includes(subtype)) return 'spreadsheet';
    return ['markdown', 'plain', 'richtext'].includes(subtype)
      ? 'document'
      : 'code';
  }
  if (top === 'application' && subtype) {
    if (subtype.includes('spreadsheet') || subtype.endsWith('ms-excel'))
      return 'spreadsheet';
    if (subtype.includes('presentation') || subtype.endsWith('ms-powerpoint'))
      return 'presentation';
    if (
      subtype.includes('wordprocessing') ||
      subtype.endsWith('msword') ||
      subtype === 'pdf'
    )
      return 'document';
    if (
      ['zip', 'tar', 'compressed'].some((fragment) =>
        subtype.includes(fragment),
      ) ||
      ['gzip', 'x-7z-compressed', 'x-bzip2', 'x-rar'].includes(subtype)
    )
      return 'archive';
    if (
      ['json', 'xml', 'javascript', 'x-yaml', 'yaml', 'sql'].includes(
        subtype,
      ) ||
      subtype.endsWith('+json') ||
      subtype.endsWith('+xml')
    )
      return 'code';
  }
  return 'unsupported';
}

function hasControlCharacter(value) {
  for (const character of value) {
    const code = character.charCodeAt(0);
    if (code < 32 || code === 127) return true;
  }
  return false;
}

function validName(value) {
  if (
    typeof value !== 'string' ||
    !value.trim() ||
    value.length > 255 ||
    /[/\\]/.test(value) ||
    hasControlCharacter(value) ||
    ['.', '..'].includes(value)
  ) {
    throw error(
      400,
      'Choose a file name without slashes or control characters.',
    );
  }
  return value.trim();
}

async function readBody(request, limit = 65536) {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    if (size > limit)
      throw error(
        413,
        `Local preview uploads are limited to ${limit / 1024 / 1024} MiB.`,
      );
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

async function readJson(request) {
  const body = await readBody(request);
  if (!body.length) return {};
  try {
    return JSON.parse(body.toString('utf8'));
  } catch {
    throw error(400, 'Invalid JSON.');
  }
}

export function createPreviewServer({
  signedOut = false,
  now = Date.now(),
} = {}) {
  const entries = new Map();
  const contents = new Map();
  const grants = new Map();
  const links = new Map();
  const versions = new Map();
  const logs = new Map();
  const operations = new Map();
  let signedIn = !signedOut;
  let uploads = 74219315;
  const ago = (hours) => new Date(now - hours * 3600000).toISOString();
  const timestamp = () => new Date().toISOString();
  const location = (path) =>
    `/org/${ORG}/${path.split('/').map(encodeURIComponent).join('/')}`;
  const page = (items) => ({ items, next_cursor: null });
  const contextId = randomUUID();
  const session = () => ({
    context_id: contextId,
    contexts: signedIn
      ? [{ context_id: contextId, org: ORG, actor: ACTOR }]
      : [],
    authenticated: signedIn,
    org: ORG,
    organizations: [ORG],
    actor: ACTOR,
    testing: false,
  });
  function add(id, path, type = 'folder', options = {}) {
    const entry = {
      id,
      name: path.split('/').at(-1),
      path,
      type,
      root_type: 'private',
      size: type === 'folder' ? null : 0,
      updated_at: ago(3),
      content_type: null,
      render:
        type === 'file'
          ? renderKind(path.split('/').at(-1), options.content_type)
          : null,
      effective_access: [...ACCESS],
      visibility: 'private',
      owner: { type: ACTOR.type, id: ACTOR.public_id },
      permanent_url: location(path),
      deleted_at: null,
      self_destruct_at: null,
      ...options,
    };
    entries.set(id, entry);
    grants.set(id, []);
    logs.set(id, [
      {
        action: type === 'folder' ? 'entry.created' : 'file.uploaded',
        actor_type: 'carbon',
        actor_id: 'c:preview',
        metadata: {},
        app_id: null,
        occurred_at: entry.updated_at,
      },
    ]);
    if (type === 'file')
      versions.set(
        id,
        [3, 2, 1].map((number) => ({
          id: `${id}-v${number}`,
          number,
          sha256: createHash('sha256').update(`${id}-${number}`).digest('hex'),
          source: number === 1 ? 'upload' : 'update',
          size: entry.size,
          created_at: ago(3 + (3 - number) * 24),
          created_by: { id: 'c:preview' },
        })),
      );
    return entry;
  }
  add('design', 'Design');
  add('team', 'Team documents', 'folder', { updated_at: ago(28) });
  add('archive', 'Archive', 'folder', { updated_at: ago(100) });
  add('campaign', CAMPAIGN);
  add('identity', 'Design/Brand identity', 'folder', { updated_at: ago(52) });
  add('assets', `${CAMPAIGN}/Assets`, 'folder', { updated_at: ago(1) });
  add('source-files', `${CAMPAIGN}/Source files`, 'folder', {
    updated_at: ago(5),
  });
  const pdf = previewPdf();
  add('brand-guide', `${CAMPAIGN}/Brand guide.pdf`, 'file', {
    content_type: 'application/pdf',
    size: 2457600,
    updated_at: ago(2),
  });
  contents.set('brand-guide', { data: pdf, type: 'application/pdf' });
  add('campaign-cover', `${CAMPAIGN}/Campaign cover.png`, 'file', {
    content_type: 'image/png',
    size: 3879731,
    updated_at: ago(1),
  });
  contents.set('campaign-cover', {
    asset: 'architecture.png',
    type: 'image/png',
  });
  add('launch-film', `${CAMPAIGN}/Launch film.mp4`, 'file', {
    content_type: 'video/mp4',
    size: 18481152,
    updated_at: ago(5),
  });
  contents.set('launch-film', {
    asset: 'launch-preview.mp4',
    type: 'video/mp4',
  });
  add('website-copy', `${CAMPAIGN}/Website copy.md`, 'file', {
    content_type: 'text/markdown',
    size: Buffer.byteLength(copy),
    updated_at: ago(4),
  });
  contents.set('website-copy', {
    data: Buffer.from(copy),
    type: 'text/markdown; charset=utf-8',
  });
  add('moodboard', `${CAMPAIGN}/Assets/Architecture study.png`, 'file', {
    content_type: 'image/png',
    size: 4183040,
    updated_at: ago(8),
  });
  contents.set('moodboard', {
    asset: 'architecture.png',
    type: 'image/png',
  });
  add('welcome', 'Team documents/Welcome.md', 'file', {
    content_type: 'text/markdown',
    size: 103,
    updated_at: ago(28),
  });
  contents.set('welcome', {
    data: Buffer.from(
      '# Welcome to the studio\n\nA shared home for thoughtful work. This is a local design-preview fixture.\n',
    ),
    type: 'text/markdown; charset=utf-8',
  });
  add('old-copy', `${CAMPAIGN}/Website copy - draft.md`, 'file', {
    content_type: 'text/markdown',
    size: 304,
    deleted_at: ago(26),
    updated_at: ago(30),
  });
  contents.set('old-copy', {
    data: Buffer.from(
      '# Earlier draft\n\nA local preview fixture you can restore from the bin.\n',
    ),
    type: 'text/markdown; charset=utf-8',
  });
  grants.set('brand-guide', [
    {
      id: 'grant-design-team',
      principal: { type: 'tag', id: 'design' },
      access: ['read'],
      inherit: false,
      expires_at: null,
    },
  ]);
  const notifications = [
    {
      id: 'notice-brand-guide',
      kind: 'access_granted',
      read: false,
      actor: { type: 'carbon', id: 'c:preview' },
      subject: {
        entry_id: 'brand-guide',
        name: 'Brand guide.pdf',
        path: `${CAMPAIGN}/Brand guide.pdf`,
      },
      access: ['read'],
      decision: null,
      created_at: ago(2),
    },
  ];

  function entryById(id, includeDeleted = false) {
    const entry = entries.get(id);
    if (!entry || (!includeDeleted && entry.deleted_at))
      throw error(404, 'File not found.');
    return entry;
  }
  function entryByPath(path) {
    const entry = [...entries.values()].find(
      (item) => item.path === path && !item.deleted_at,
    );
    if (!entry) throw error(404, 'File not found.');
    return entry;
  }
  function folderAt(path) {
    if (!path) return null;
    const folder = entryByPath(path);
    if (folder.type !== 'folder')
      throw error(400, 'Choose a destination folder.');
    return folder;
  }
  function uniquePath(path, exceptId) {
    if (
      [...entries.values()].some(
        (item) =>
          item.path === path && !item.deleted_at && item.id !== exceptId,
      )
    )
      throw error(409, 'An item with that name already exists here.');
  }
  function children(path) {
    return [...entries.values()].filter(
      (entry) =>
        !entry.deleted_at &&
        entry.path.split('/').slice(0, -1).join('/') === path,
    );
  }
  function record(entry, action, metadata = {}) {
    entry.updated_at = timestamp();
    logs.get(entry.id).unshift({
      action,
      actor_type: 'carbon',
      actor_id: ACTOR.public_id,
      metadata,
      app_id: null,
      occurred_at: entry.updated_at,
    });
  }
  function linkAccess(entry) {
    const link = links.get(entry.id);
    const enabled =
      !!link?.enabled &&
      (!link.expires_at || Date.parse(link.expires_at) > Date.now());
    return {
      can_manage: true,
      enabled,
      effective: enabled,
      inherited_from: null,
      url: enabled ? location(entry.path) : null,
      expires_at: link?.expires_at || null,
    };
  }
  function expires(minutes) {
    if (minutes === undefined) return null;
    if (!Number.isInteger(minutes) || minutes < 1 || minutes > 43200)
      throw error(400, 'Choose a time between 1 minute and 30 days.');
    return new Date(Date.now() + minutes * 60000).toISOString();
  }
  const publicShape = (entry) => ({
    id: entry.id,
    name: entry.name,
    path: entry.path,
    entry_type: entry.type,
    content_type: entry.content_type,
    size: entry.size,
  });
  function json(response, status, value) {
    response.writeHead(status, {
      'Content-Type': 'application/json; charset=utf-8',
    });
    response.end(JSON.stringify(value));
  }
  async function sendContent(request, response, entry, attachment = false) {
    if (entry.type === 'folder')
      throw error(
        501,
        'Folder archive downloads are not simulated in the local design preview.',
      );
    const content = contents.get(entry.id);
    if (!content) throw error(404, 'This preview fixture has no content.');
    let data = content.data;
    const type = content.type;
    if (content.asset) {
      try {
        data = await readFile(new URL(content.asset, ASSETS));
      } catch {
        throw error(404, 'This local preview asset is unavailable.');
      }
    }
    const headers = {
      'Content-Type': type,
      'Accept-Ranges': 'bytes',
      'Content-Disposition': `${attachment ? 'attachment' : 'inline'}; filename*=UTF-8''${encodeURIComponent(entry.name)}`,
    };
    if (request.headers.range) {
      const match = /^bytes=(\d+)-(\d*)$/.exec(request.headers.range);
      const start = match ? Number(match[1]) : NaN;
      const end =
        match && match[2]
          ? Math.min(Number(match[2]), data.length - 1)
          : data.length - 1;
      if (!Number.isSafeInteger(start) || start >= data.length || end < start) {
        response.writeHead(416, {
          ...headers,
          'Content-Range': `bytes */${data.length}`,
        });
        response.end();
        return;
      }
      response.writeHead(206, {
        ...headers,
        'Content-Length': end - start + 1,
        'Content-Range': `bytes ${start}-${end}/${data.length}`,
      });
      response.end(
        request.method === 'HEAD' ? undefined : data.subarray(start, end + 1),
      );
      return;
    }
    response.writeHead(200, { ...headers, 'Content-Length': data.length });
    response.end(request.method === 'HEAD' ? undefined : data);
  }

  return createServer(async (request, response) => {
    response.setHeader('Cache-Control', 'no-store');
    response.setHeader('X-Content-Type-Options', 'nosniff');
    response.setHeader('X-Briefcase-Design-Preview', 'local-fixtures-only');
    try {
      const host = new URL(`http://${request.headers.host || ''}`);
      if (
        !isLoopback(host.hostname) ||
        (request.headers.origin &&
          !isLoopback(new URL(request.headers.origin).hostname))
      )
        throw error(
          403,
          'The design preview only accepts local browser requests.',
        );
      const url = new URL(request.url, host);
      const path = url.pathname.replace(/\/$/, '');
      const method = request.method;
      const params = url.searchParams;
      if (path === '/browser/session') {
        if (method === 'DELETE') signedIn = false;
        else if (method === 'PATCH') {
          const body = await readJson(request);
          if (!signedIn) throw error(401, 'Sign in to the local preview.');
          if (body.context_id !== contextId)
            throw error(403, 'Only the design-preview workspace exists here.');
        } else if (method !== 'GET') throw error(405, 'Method not supported.');
        json(response, 200, session());
        return;
      }
      if (path === '/browser/login/start' && method === 'POST') {
        const body = await readJson(request);
        signedIn = true;
        const returnTo =
          typeof body.return_to === 'string' &&
          /^\/(?:org\/design-preview(?:\/|$))/.test(body.return_to) &&
          !/[\\\r\n]/.test(body.return_to)
            ? body.return_to
            : '/';
        json(response, 200, { redirect_url: returnTo });
        return;
      }
      if (
        path === '/browser/public' &&
        (method === 'GET' || method === 'HEAD')
      ) {
        if (params.get('org') !== ORG) throw error(404, 'File not found.');
        const entry = entryByPath(params.get('path'));
        // Private deep links enter the real workspace UI via its existing 404 path.
        if (
          !linkAccess(entry).effective &&
          ![...entries.values()].some(
            (parent) =>
              parent.type === 'folder' &&
              entry.path.startsWith(parent.path + '/') &&
              linkAccess(parent).effective,
          )
        )
          throw error(404, 'File not found.');
        const view = params.get('view') || 'metadata';
        if (view === 'metadata') json(response, 200, publicShape(entry));
        else if (view === 'contents')
          json(response, 200, page(children(entry.path).map(publicShape)));
        else if (view === 'inline' || view === 'attachment')
          await sendContent(request, response, entry, view === 'attachment');
        else throw error(400, 'Unknown public view.');
        return;
      }
      if (!signedIn) throw error(401, 'Sign in to the local preview.');
      const org = request.headers['x-briefcase-organization'];
      if (org && org !== ORG)
        throw error(403, 'Only the design-preview workspace exists here.');
      if (params.has('test_environment'))
        throw error(
          400,
          'Testing credentials are not simulated in the local design preview.',
        );
      if (path === '/browser/entries' && method === 'GET') {
        const folder = params.get('path') || '';
        folderAt(folder);
        const filter = params.get('filter') || '';
        let items = filter.includes('last:')
          ? [...entries.values()]
              .filter((entry) => entry.type === 'file' && !entry.deleted_at)
              .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
              .slice(0, 20)
          : children(folder);
        if (filter && !filter.includes('last:')) {
          const needle = filter
            .replace(/(?:sort:\w+|type:\w+)/g, '')
            .trim()
            .toLowerCase();
          if (needle)
            items = items.filter((entry) =>
              entry.name.toLowerCase().includes(needle),
            );
          if (filter.includes('type:file'))
            items = items.filter((entry) => entry.type === 'file');
          if (filter.includes('type:folder'))
            items = items.filter((entry) => entry.type === 'folder');
        }
        json(response, 200, page(items));
        return;
      }
      if (path === '/browser/resolve' && method === 'GET') {
        json(response, 200, entryByPath(params.get('path')));
        return;
      }
      if (path === '/browser/search' && method === 'GET') {
        const q = (params.get('q') || '').toLowerCase();
        json(
          response,
          200,
          [...entries.values()]
            .filter(
              (entry) =>
                !entry.deleted_at && entry.name.toLowerCase().includes(q),
            )
            .map((entry) => ({ entry })),
        );
        return;
      }
      if (path === '/browser/usage' && method === 'GET') {
        const used =
          12884901888 +
          [...entries.values()].reduce(
            (sum, entry) => sum + (entry.size || 0),
            0,
          );
        const reset = new Date();
        reset.setUTCHours(24, 0, 0, 0);
        json(response, 200, {
          storage: {
            used_bytes: used,
            limit_bytes: 107374182400,
            remaining_bytes: 107374182400 - used,
          },
          daily_uploads: {
            used_bytes: uploads,
            limit_bytes: 5368709120,
            remaining_bytes: 5368709120 - uploads,
            resets_at: reset.toISOString(),
          },
        });
        return;
      }
      if (path === '/browser/bin' && method === 'GET') {
        json(
          response,
          200,
          page([...entries.values()].filter((entry) => entry.deleted_at)),
        );
        return;
      }
      if (path === '/browser/notifications' && method === 'GET') {
        json(response, 200, {
          items: notifications,
          unread_count: notifications.filter((notice) => !notice.read).length,
        });
        return;
      }
      if (path === '/browser/notifications/read' && method === 'POST') {
        notifications.forEach((notice) => {
          notice.read = true;
        });
        json(response, 200, { items: notifications, unread_count: 0 });
        return;
      }
      if (path === '/browser/storage/configuration')
        throw error(
          501,
          'Storage configuration is unavailable in the local design preview. No AWS request was made.',
        );
      if (path.startsWith('/browser/environments'))
        throw error(
          501,
          'Testing environments are unavailable in the local design preview.',
        );

      const upload = path === '/browser/upload' && method === 'POST';
      const body = upload
        ? null
        : ['POST', 'PATCH', 'PUT', 'DELETE'].includes(method)
          ? await readJson(request)
          : {};
      const operation = upload ? params.get('operation_id') : body.operation_id;
      const operationKey = operation ? `${method}:${path}:${operation}` : null;
      if (operationKey && operations.has(operationKey)) {
        json(response, 200, operations.get(operationKey));
        return;
      }
      let result;
      if ((path === '/browser/entries' && method === 'POST') || upload) {
        const parent = upload ? params.get('parent') || '' : body.parent || '';
        const name = validName(upload ? params.get('name') : body.name);
        if (upload && name === 'rejected.txt')
          throw error(
            413,
            'This file was rejected by the local preview upload fixture.',
          );
        folderAt(parent);
        const nextPath = parent ? `${parent}/${name}` : name;
        uniquePath(nextPath);
        const data = upload ? await readBody(request, MAX_UPLOAD) : null;
        const minutes =
          upload && params.has('self_destruct_minutes')
            ? Number(params.get('self_destruct_minutes'))
            : undefined;
        const selfDestruct = expires(minutes);
        result = add(randomUUID(), nextPath, upload ? 'file' : 'folder', {
          root_type: body?.root_type || 'private',
          tag: body?.tag,
          updated_at: timestamp(),
          ...(upload
            ? {
                size: data.length,
                content_type:
                  params.get('content_type') || 'application/octet-stream',
                self_destruct_at: selfDestruct,
              }
            : {}),
        });
        if (upload) {
          contents.set(result.id, { data, type: result.content_type });
          uploads += data.length;
        }
      } else if (
        /^\/browser\/bin\/[^/]+\/restore$/.test(path) &&
        method === 'POST'
      ) {
        const entry = entryById(path.split('/')[3], true);
        uniquePath(entry.path, entry.id);
        const deletedAt = entry.deleted_at;
        for (const item of entries.values())
          if (
            item.id === entry.id ||
            (item.path.startsWith(entry.path + '/') &&
              item.deleted_at === deletedAt)
          )
            item.deleted_at = null;
        record(entry, 'entry.restored');
        result = entry;
      } else {
        const match = /^\/browser\/entries\/([^/]+)(?:\/(.*))?$/.exec(path);
        if (!match)
          throw error(
            404,
            'This endpoint is not part of the local design preview.',
          );
        const entry = entryById(decodeURIComponent(match[1]));
        const action = match[2] || '';
        if (!action && method === 'GET') result = entry;
        else if (
          (action === 'content' || action === 'download') &&
          (method === 'GET' || method === 'HEAD')
        ) {
          await sendContent(
            request,
            response,
            entry,
            action === 'download' ||
              ['1', 'true'].includes(params.get('download')),
          );
          return;
        } else if (!action && method === 'PATCH') {
          const parent = body.parent_id
            ? entryById(body.parent_id)
            : folderAt(entry.path.split('/').slice(0, -1).join('/'));
          if (parent?.type !== 'folder' && parent)
            throw error(400, 'Choose a destination folder.');
          if (
            parent &&
            (parent.id === entry.id || parent.path.startsWith(entry.path + '/'))
          )
            throw error(400, 'A folder cannot be moved into itself.');
          const name =
            body.name === undefined ? entry.name : validName(body.name);
          const oldPath = entry.path;
          const newPath = parent ? `${parent.path}/${name}` : name;
          uniquePath(newPath, entry.id);
          for (const item of entries.values())
            if (item.id === entry.id || item.path.startsWith(oldPath + '/')) {
              item.path = newPath + item.path.slice(oldPath.length);
              item.permanent_url = location(item.path);
            }
          entry.name = name;
          if (entry.type === 'file')
            entry.render = renderKind(name, entry.content_type);
          record(
            entry,
            body.name === undefined ? 'entry.moved' : 'entry.renamed',
          );
          result = entry;
        } else if (!action && method === 'DELETE') {
          const removedAt = timestamp();
          for (const item of entries.values())
            if (
              item.id === entry.id ||
              item.path.startsWith(entry.path + '/')
            ) {
              if (item.self_destruct_at) {
                entries.delete(item.id);
                contents.delete(item.id);
              } else item.deleted_at ||= removedAt;
            }
          result = { deleted: true };
        } else if (action === 'self-destruct' && method === 'DELETE') {
          entry.self_destruct_at = null;
          record(entry, 'self_destruct.cancelled');
          result = entry;
        } else if (action === 'versions' && method === 'GET')
          result = page(versions.get(entry.id) || []);
        else if (
          /^versions\/[^/]+\/restore$/.test(action) &&
          method === 'POST'
        ) {
          const previous = (versions.get(entry.id) || []).find(
            (version) => version.id === action.split('/')[1],
          );
          if (!previous) throw error(404, 'Version not found.');
          versions.get(entry.id).unshift({
            ...previous,
            id: randomUUID(),
            number: versions.get(entry.id)[0].number + 1,
            source: 'restore',
            created_at: timestamp(),
          });
          record(entry, 'version.restored');
          result = entry;
        } else if (action === 'logs' && method === 'GET')
          result = page(logs.get(entry.id));
        else if (action === 'link-access' && method === 'GET')
          result = linkAccess(entry);
        else if (action === 'link-access' && method === 'PUT') {
          links.set(entry.id, {
            enabled: !!body.enabled,
            expires_at: expires(body.expires_in_minutes),
          });
          record(entry, 'link_access.updated');
          result = linkAccess(entry);
        } else if (action === 'invitations' && method === 'GET')
          result = page(grants.get(entry.id));
        else if (action === 'invitations' && method === 'POST') {
          if (
            !body.principal?.id ||
            !['carbon', 'silicon', 'email', 'tag'].includes(
              body.principal?.type,
            )
          )
            throw error(400, 'Choose a valid share recipient.');
          result = {
            id: randomUUID(),
            principal: body.principal,
            access: body.access || ['read'],
            inherit: !!body.inherit,
            expires_at: expires(body.expires_in_minutes),
          };
          grants.get(entry.id).push(result);
          record(entry, 'access.granted', { principal: body.principal });
        } else if (
          /^invitations\/[^/]+$/.test(action) &&
          ['PATCH', 'DELETE'].includes(method)
        ) {
          const values = grants.get(entry.id);
          const index = values.findIndex(
            (grant) => grant.id === action.split('/')[1],
          );
          if (index < 0) throw error(404, 'Share not found.');
          if (method === 'DELETE') {
            values.splice(index, 1);
            result = { revoked: true };
          } else {
            values[index].expires_at = body.permanent
              ? null
              : expires(body.expires_in_minutes);
            result = values[index];
          }
          record(entry, 'access.updated');
        } else
          throw error(
            405,
            'This action is not simulated by the local design preview.',
          );
      }
      if (operationKey) operations.set(operationKey, structuredClone(result));
      if (
        upload &&
        operationKey &&
        params.get('name') === 'recover-on-retry.txt'
      ) {
        // The response is intentionally lost only after the result is saved.
        // An identical retry returns from the operation cache above.
        request.socket.destroy();
        return;
      }
      json(response, 200, result);
    } catch (failure) {
      if (!response.headersSent)
        json(response, failure.status || 500, {
          error: {
            message: failure.status
              ? failure.message
              : 'The local preview could not complete this request.',
          },
        });
      else response.end();
    }
  });
}

if (
  process.argv[1] &&
  pathToFileURL(fileURLToPath(import.meta.url)).href ===
    pathToFileURL(process.argv[1]).href
) {
  const portFlag = process.argv.indexOf('--port');
  const port = portFlag < 0 ? 4328 : Number(process.argv[portFlag + 1]);
  if (!Number.isInteger(port) || port < 1 || port > 65535)
    throw new Error('Use --port with a port from 1 to 65535.');
  const server = createPreviewServer({
    signedOut: process.argv.includes('--signed-out'),
  });
  server.listen(port, '127.0.0.1', () => {
    console.log(`Local design preview: http://127.0.0.1:${port}`);
    console.log(
      'In-memory fixtures only. No IAM, S3, or production connection. Restart to reset.',
    );
  });
}
