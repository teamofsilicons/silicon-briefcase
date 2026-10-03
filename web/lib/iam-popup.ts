export type IdentityKind = 'carbon' | 'silicon';
export class PopupBlockedError extends Error {
  constructor() {
    super('The sign-in popup was blocked. Continue in this tab instead.');
    this.name = 'PopupBlockedError';
  }
}

export async function continueIamInThisTab(
  start: () => string | Promise<string>,
  signal?: AbortSignal,
): Promise<void> {
  if (signal?.aborted) throw new Error('Sign-in cancelled.');
  const url = await start();
  if (signal?.aborted) throw new Error('Sign-in cancelled.');
  window.location.assign(url);
}

const messageType = 'silicon:iam-login-complete';
const validContext = (value: unknown): value is string =>
  typeof value === 'string' &&
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);

// Completion carries a nonce and public candidate receipt. Only explicit
// server activation can select it; messages never contain credentials.
export function completeIamPopup(): boolean {
  const url = new URL(window.location.href);
  if (url.searchParams.get('iam_popup') !== 'complete') return false;
  const nonce = url.searchParams.get('nonce');
  const result = url.searchParams.get('result');
  const context = url.searchParams.get('context');
  history.replaceState(null, '', url.pathname + url.hash);
  if (
    nonce &&
    /^[a-f0-9]{64}$/.test(nonce) &&
    ['ok', 'error'].includes(result || '') &&
    (result !== 'ok' || validContext(context)) &&
    window.opener
  ) {
    window.opener.postMessage(
      { type: messageType, nonce, result, context },
      window.location.origin,
    );
    window.close();
  }
  return true;
}

export function openIamPopup(
  start: (nonce: string) => string | Promise<string>,
  signal?: AbortSignal,
): Promise<string> {
  if (signal?.aborted) return Promise.reject(new Error('Sign-in cancelled.'));
  const nonce = Array.from(
    crypto.getRandomValues(new Uint8Array(32)),
    (value) => value.toString(16).padStart(2, '0'),
  ).join('');
  let popup: Window | null;
  try {
    popup = window.open(
      'about:blank',
      'iam-' + nonce,
      'popup,width=520,height=760',
    );
  } catch {
    return Promise.reject(new PopupBlockedError());
  }
  if (!popup) return Promise.reject(new PopupBlockedError());
  return new Promise((resolve, reject) => {
    let settled = false;
    const finish = (error?: Error, context?: string) => {
      if (settled) return;
      settled = true;
      window.removeEventListener('message', receive);
      signal?.removeEventListener('abort', cancel);
      clearInterval(closed);
      clearTimeout(timeout);
      popup.close();
      if (error) reject(error);
      else resolve(context!);
    };
    const cancel = () => finish(new Error('Sign-in cancelled.'));
    const receive = (event: MessageEvent) => {
      if (
        event.origin !== window.location.origin ||
        event.source !== popup ||
        event.data?.type !== messageType ||
        event.data?.nonce !== nonce
      )
        return;
      if (event.data.result === 'ok' && validContext(event.data.context))
        finish(undefined, event.data.context);
      else if (event.data.result === 'error')
        finish(new Error('IAM could not finish sign-in. Please try again.'));
    };
    const closed = setInterval(() => {
      if (popup.closed)
        finish(
          new Error(
            'Sign-in was closed. Choose your account type to try again.',
          ),
        );
    }, 500);
    const timeout = setTimeout(
      () =>
        finish(
          new Error('Sign-in expired. Choose your account type to try again.'),
        ),
      600_000,
    );
    window.addEventListener('message', receive);
    signal?.addEventListener('abort', cancel, { once: true });
    Promise.resolve()
      .then(() => start(nonce))
      .then((url) => {
        if (!settled) popup.location.href = url;
      })
      .catch((error) =>
        finish(
          error instanceof Error
            ? error
            : new Error('Unable to start sign-in.'),
        ),
      );
  });
}

/** Verify server return paths again before browser navigation. */
export function safeLoginReturn(value: unknown): string {
  if (
    typeof value !== 'string' ||
    (value !== '/' && !value.startsWith('/org/'))
  )
    throw new Error('Invalid sign-in return path.');
  const parsed = new URL(value, window.location.origin);
  if (
    parsed.origin !== window.location.origin ||
    parsed.pathname !== value ||
    parsed.search ||
    parsed.hash
  )
    throw new Error('Invalid sign-in return path.');
  return value;
}
