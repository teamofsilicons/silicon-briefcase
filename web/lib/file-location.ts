export type FileLocation = { org: string; path: string };

// Decode each component independently so an encoded slash cannot change the
// organisation or folder boundary after validation.
export function readFileLocation(
  pathname = window.location.pathname,
): FileLocation | null {
  if (pathname === '/') return null;
  if (!pathname.startsWith('/org/')) throw new Error('File not found.');
  const parts = pathname
    .slice(5)
    .replace(/\/$/, '')
    .split('/')
    .map((part) => {
      let decoded: string;
      try {
        decoded = decodeURIComponent(part);
      } catch {
        throw new Error('File not found.');
      }
      if (
        !decoded ||
        decoded === '.' ||
        decoded === '..' ||
        decoded.includes('/') ||
        decoded.includes('\\') ||
        hasControlCharacter(decoded)
      ) {
        throw new Error('File not found.');
      }
      return decoded;
    });
  return { org: parts[0], path: parts.slice(1).join('/') };
}

export function fileLocation(org: string, path = ''): string {
  return (
    '/org/' +
    encodeURIComponent(org) +
    '/' +
    path.split('/').filter(Boolean).map(encodeURIComponent).join('/')
  );
}

function hasControlCharacter(value: string): boolean {
  for (const character of value) {
    const code = character.charCodeAt(0);
    if (code < 32 || code === 127) return true;
  }
  return false;
}
