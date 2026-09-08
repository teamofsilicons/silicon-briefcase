import {
  escapeMarkup,
  previewDocument,
  type TextFormat,
} from './preview-document';
// Vite generates this default constructor; it is not an export of the source.
// oxlint-disable-next-line import/default
import PreviewWorker from './preview.worker.ts?worker';

export async function richPreview(
  text: string,
  format: TextFormat,
  signal: AbortSignal,
): Promise<{ document: string; note: string }> {
  signal.throwIfAborted();
  const result = await new Promise<{ body: string; note: string }>(
    (resolve, reject) => {
      const sourcePreview = () => ({
        body: `<pre><code>${escapeMarkup(text)}</code></pre>`,
        note: 'Formatted preview is unavailable for this excerpt. Showing its source.',
      });
      let worker: Worker;
      try {
        // Let Vite resolve the browser asset without an SSR import.meta URL.
        worker = new PreviewWorker();
      } catch {
        resolve(sourcePreview());
        return;
      }
      const cleanup = () => {
        clearTimeout(timer);
        signal.removeEventListener('abort', abort);
        worker.terminate();
      };
      const fallback = () => {
        cleanup();
        resolve(sourcePreview());
      };
      const abort = () => {
        cleanup();
        reject(signal.reason);
      };
      const timer = setTimeout(fallback, 10000);
      signal.addEventListener('abort', abort, { once: true });
      worker.onerror = fallback;
      worker.onmessage = (
        event: MessageEvent<{ body?: string; note?: string; error?: boolean }>,
      ) => {
        if (
          event.data.error ||
          typeof event.data.body !== 'string' ||
          event.data.body.length > 8 * 1024 * 1024
        ) {
          fallback();
          return;
        }
        cleanup();
        resolve({ body: event.data.body, note: event.data.note || '' });
      };
      worker.postMessage({ text, format });
    },
  );
  signal.throwIfAborted();
  return { document: await previewDocument(result.body), note: result.note };
}
