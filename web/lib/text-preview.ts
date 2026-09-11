import { ApiError, browserUrl } from './api';

const PREVIEW_BYTES = 1024 * 1024;

export async function textPreview(
  id: string,
  signal: AbortSignal,
  emptyHint = false,
): Promise<{ text: string; truncated: boolean }> {
  // Ask for one byte beyond the display budget to distinguish an exact fit.
  // Independently cap the reader even if a proxy ignores Range or the entry
  // changes between metadata lookup and this request.
  const response = await fetch(
    browserUrl('/browser/entries/' + encodeURIComponent(id) + '/content'),
    {
      signal,
      credentials: 'same-origin',
      redirect: 'error',
      // A zero-byte file has no satisfiable range. Still fetch it so stale
      // metadata cannot suppress the current content/authorization check.
      headers: emptyHint ? {} : { Range: `bytes=0-${PREVIEW_BYTES}` },
    },
  );
  if (!response.ok)
    throw new ApiError('Preview is unavailable.', response.status);
  if (!response.body) return { text: '', truncated: false };
  const reader = response.body.getReader();
  const buffer = new Uint8Array(PREVIEW_BYTES);
  let length = 0,
    truncated = false;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      const remaining = PREVIEW_BYTES - length;
      buffer.set(value.subarray(0, remaining), length);
      length += Math.min(remaining, value.length);
      if (value.length > remaining) {
        truncated = true;
        break;
      }
    }
    const contentType = response.headers.get('content-type') || '';
    const charset = /(?:^|;)\s*charset\s*=\s*"?([^;"\s]+)/i.exec(
      contentType,
    )?.[1];
    // BOMs can span transport chunks; decode only after assembling the bounded
    // excerpt. UTF-16 spreadsheets and source files are common desktop exports.
    const encoding =
      length >= 2 && buffer[0] === 0xff && buffer[1] === 0xfe
        ? 'utf-16le'
        : length >= 2 && buffer[0] === 0xfe && buffer[1] === 0xff
          ? 'utf-16be'
          : charset || 'utf-8';
    let decoder: TextDecoder;
    try {
      decoder = new TextDecoder(encoding);
    } catch {
      decoder = new TextDecoder();
    }
    return { text: decoder.decode(buffer.subarray(0, length)), truncated };
  } finally {
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}
