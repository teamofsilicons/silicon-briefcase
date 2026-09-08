export type TextMode = 'markdown' | 'csv' | 'tsv' | 'code' | 'text';
export type TextFormat = { mode: TextMode; language?: string };

export function textFormat(entry: {
  name: string;
  content_type: string | null;
  render: string | null;
}): TextFormat | null {
  const extension = entry.name.split('.').pop()?.toLowerCase() || '';
  const mime = entry.content_type?.split(';')[0].trim().toLowerCase();
  if (
    ['md', 'markdown', 'mdown'].includes(extension) ||
    mime === 'text/markdown'
  )
    return { mode: 'markdown' };
  if (extension === 'csv' || mime === 'text/csv') return { mode: 'csv' };
  if (extension === 'tsv' || mime === 'text/tab-separated-values')
    return { mode: 'tsv' };
  const languages: Record<string, string> = {
    json: 'json',
    jsonl: 'json',
    xml: 'xml',
    html: 'xml',
    htm: 'xml',
    yaml: 'yaml',
    yml: 'yaml',
    css: 'css',
    js: 'javascript',
    jsx: 'javascript',
    mjs: 'javascript',
    cjs: 'javascript',
    ts: 'typescript',
    tsx: 'typescript',
    py: 'python',
    java: 'java',
    sql: 'sql',
    rs: 'rust',
    go: 'go',
    rb: 'ruby',
    sh: 'bash',
    bash: 'bash',
    c: 'c',
    h: 'c',
    cpp: 'cpp',
    hpp: 'cpp',
    cs: 'csharp',
    php: 'php',
    swift: 'swift',
    kt: 'kotlin',
    kts: 'kotlin',
    ini: 'ini',
    toml: 'ini',
    diff: 'diff',
    patch: 'diff',
    tex: 'latex',
    lua: 'lua',
  };
  if (languages[extension])
    return { mode: 'code', language: languages[extension] };
  if (mime === 'application/json') return { mode: 'code', language: 'json' };
  if (mime === 'application/xml') return { mode: 'code', language: 'xml' };
  if (['txt', 'text', 'log'].includes(extension)) return { mode: 'text' };
  if (mime?.startsWith('text/') || entry.render === 'code')
    return { mode: 'text' };
  return null;
}

export function escapeMarkup(value: string): string {
  return value.replace(
    /[&<>"']/g,
    (character) =>
      ({
        '&': '&amp;',
        '<': '&lt;',
        '>': '&gt;',
        '"': '&quot;',
        "'": '&#39;',
      })[character]!,
  );
}

const styles = `
:root{color-scheme:light dark;font:16px/1.6 system-ui,sans-serif;background:light-dark(#fff,#161a20);color:light-dark(#20252c,#e5e9ef)}
body{margin:0;padding:20px;overflow-wrap:anywhere}*{box-sizing:border-box}
pre{white-space:pre;overflow:auto;margin:0;padding:12px;background:light-dark(#f5f7fa,#101419);tab-size:4}
code{font:14px/1.65 ui-monospace,SFMono-Regular,Consolas,monospace}p code,li code{padding:2px 4px;background:light-dark(#f0f2f5,#222831)}
h1,h2,h3{line-height:1.3}h1{font-size:2rem}h2{font-size:1.5rem}h3{font-size:1.25rem}
blockquote{margin-left:0;padding-left:16px;border-left:3px solid #8294ad;color:light-dark(#45566f,#b2bfce)}
table{border-collapse:collapse;font-size:14px;white-space:pre-wrap}th,td{border:1px solid light-dark(#ccd3dd,#394350);padding:8px 12px;max-width:360px;min-width:72px;text-align:left;vertical-align:top}
th{position:sticky;top:0;background:light-dark(#edf1f7,#28313e)}.row-number{min-width:40px;text-align:right;color:light-dark(#596777,#a5b6c8)}
.inert-link{color:light-dark(#1f5fb8,#8cbcff);text-decoration:underline}.image-label{font-style:italic}
.hljs-comment,.hljs-quote{color:light-dark(#586a5a,#9bab91)}
.hljs-keyword,.hljs-selector-tag,.hljs-literal,.hljs-built_in{color:light-dark(#8130a0,#db9be9)}
.hljs-string,.hljs-regexp,.hljs-addition{color:light-dark(#286837,#9dcf9b)}
.hljs-number,.hljs-symbol,.hljs-bullet,.hljs-attr,.hljs-attribute{color:light-dark(#95501b,#edb585)}
.hljs-title,.hljs-name,.hljs-type,.hljs-section{color:light-dark(#175ba8,#91bded)}
.hljs-deletion{color:light-dark(#a62c3a,#f6a1aa)}.hljs-emphasis{font-style:italic}.hljs-strong{font-weight:bold}
`;

export async function previewDocument(body: string): Promise<string> {
  const digest = await crypto.subtle.digest(
    'SHA-256',
    new TextEncoder().encode(styles),
  );
  const hash = btoa(String.fromCharCode(...new Uint8Array(digest)));
  // Only fixed application CSS is allowed. No scripts, links, forms, embeds,
  // fonts or network requests, even if a parser regresses its escaping.
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'none'; style-src 'sha256-${hash}'; base-uri 'none'; form-action 'none';"><meta name="referrer" content="no-referrer"><style>${styles}</style></head><body>${body}</body></html>`;
}
