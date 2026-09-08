import hljs from 'highlight.js/lib/common';
import latex from 'highlight.js/lib/languages/latex';
import { Marked } from 'marked';
import Papa from 'papaparse';
import { escapeMarkup, type TextFormat } from './preview-document';

hljs.registerLanguage('latex', latex);

function highlight(text: string, language?: string): string {
  return language && hljs.getLanguage(language)
    ? hljs.highlight(text, { language, ignoreIllegals: true }).value
    : escapeMarkup(text);
}

const markdown = new Marked({
  async: false,
  gfm: true,
  renderer: {
    html(token) {
      return escapeMarkup(token.text);
    },
    link(token) {
      return `<span class="inert-link">${this.parser.parseInline(token.tokens)}</span>`;
    },
    image(token) {
      return `<span class="image-label">[Image: ${escapeMarkup(token.text)}]</span>`;
    },
    code(token) {
      return `<pre><code>${highlight(token.text, token.lang?.split(/\s/)[0])}</code></pre>`;
    },
  },
});

function render(
  text: string,
  format: TextFormat,
): { body: string; note: string } {
  if (!text) return { body: '<p>Empty file</p>', note: '' };
  if (format.mode === 'markdown') {
    return {
      body: markdown.parse(text) as string,
      note: 'Links and external images are disabled in this preview. Embedded HTML is shown as source.',
    };
  }
  if (format.mode === 'csv' || format.mode === 'tsv') {
    const parsed = Papa.parse<string[]>(text, {
      delimiter: format.mode === 'tsv' ? '\t' : ',',
      dynamicTyping: false,
      header: false,
      preview: 201,
    });
    const rows = parsed.data.slice(0, 200);
    const width = Math.min(50, Math.max(0, ...rows.map((row) => row.length)));
    const column = (index: number) => {
      let name = '';
      for (let n = index + 1; n > 0; n = Math.floor((n - 1) / 26))
        name = String.fromCharCode(65 + ((n - 1) % 26)) + name;
      return name;
    };
    const header = Array.from(
      { length: width },
      (_, i) => `<th scope="col">${column(i)}</th>`,
    ).join('');
    const body = rows
      .map(
        (row, index) =>
          `<tr><th class="row-number" scope="row">${index + 1}</th>${Array.from({ length: width }, (_, i) => `<td>${escapeMarkup(row[i] || '')}</td>`).join('')}</tr>`,
      )
      .join('');
    const limited =
      parsed.data.length > 200 ||
      parsed.meta.truncated ||
      rows.some((row) => row.length > 50);
    return {
      body: `<table aria-label="Spreadsheet cells"><thead><tr><th scope="col">Row</th>${header}</tr></thead><tbody>${body}</tbody></table>`,
      note: [
        limited ? 'Showing up to 200 rows and 50 columns.' : '',
        parsed.errors.length
          ? 'This excerpt contains incomplete or malformed CSV quoting; some cells may be partial.'
          : '',
        'Values are displayed as text; formulas are never executed.',
      ]
        .filter(Boolean)
        .join(' '),
    };
  }
  return {
    body: `<pre><code>${highlight(text, format.language)}</code></pre>`,
    note: '',
  };
}

// This worker parses only bounded text supplied by the authenticated parent.
// No fetches, URLs, credentials, dynamic scripts or user code execution.
self.onmessage = (
  event: MessageEvent<{ text: string; format: TextFormat }>,
) => {
  try {
    if (event.data.text.length > 1024 * 1024)
      throw new Error('Preview input exceeds its limit.');
    const result = render(event.data.text, event.data.format);
    if (result.body.length > 8 * 1024 * 1024)
      throw new Error('Preview output exceeds its limit.');
    self.postMessage(result);
  } catch {
    self.postMessage({ error: true });
  }
};
