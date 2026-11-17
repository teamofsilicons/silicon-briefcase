/**
 * What kind of thing an entry is, for the 28px tile that leads its row and the grid's cards: a folder, an image, a
 * document… Each kind has a quiet colour of its own (styles/tokens.css, --file-*), drawn on a 10% tint of itself.
 */
import type { CSSProperties } from "react";
import { Archive, File, FileCode2, FileSpreadsheet, FileText, Folder, Image, Music, Presentation, Video, type LucideIcon } from "lucide-react";
import type { Entry } from "@/lib/briefcase/types";
import styles from "./files.module.css";

type Kind = { label: string; icon: LucideIcon; tone: string };

const KINDS = {
  folder: { label: "Folder", icon: Folder, tone: "var(--file-folder)" },
  image: { label: "Image", icon: Image, tone: "var(--file-image)" },
  video: { label: "Video", icon: Video, tone: "var(--file-video)" },
  audio: { label: "Audio", icon: Music, tone: "var(--file-audio)" },
  spreadsheet: { label: "Spreadsheet", icon: FileSpreadsheet, tone: "var(--file-spreadsheet)" },
  presentation: { label: "Presentation", icon: Presentation, tone: "var(--file-presentation)" },
  archive: { label: "Archive", icon: Archive, tone: "var(--file-archive)" },
  code: { label: "Code", icon: FileCode2, tone: "var(--file-code)" },
  document: { label: "Document", icon: FileText, tone: "var(--file-document)" },
  file: { label: "File", icon: File, tone: "var(--file-archive)" },
} satisfies Record<string, Kind>;

const EXTENSIONS: Array<[keyof typeof KINDS, RegExp]> = [
  ["spreadsheet", /\.(csv|tsv|xlsx?|ods|numbers)$/i],
  ["presentation", /\.(pptx?|odp|key)$/i],
  ["archive", /\.(zip|tar|gz|tgz|zst|7z|rar|bz2|xz)$/i],
  ["code", /\.(js|mjs|ts|tsx|jsx|py|rs|go|rb|java|kt|swift|c|h|cpp|cs|sh|json|ya?ml|toml|html|css|sql)$/i],
  ["document", /\.(pdf|docx?|odt|rtf|txt|md|markdown|pages)$/i],
];

export function kindOf(entry: Pick<Entry, "type" | "name" | "content_type">): Kind {
  if (entry.type === "folder") return KINDS.folder;
  const type = entry.content_type ?? "";
  if (type.startsWith("image/")) return KINDS.image;
  if (type.startsWith("video/")) return KINDS.video;
  if (type.startsWith("audio/")) return KINDS.audio;
  const byName = EXTENSIONS.find(([, pattern]) => pattern.test(entry.name));
  if (byName) return KINDS[byName[0]];
  if (type.startsWith("text/") || type === "application/pdf") return KINDS.document;
  return KINDS.file;
}

/** The kind's tile: 28px in a row, `large` (40px) on a card. */
export function FileTile({ entry, large }: { entry: Pick<Entry, "type" | "name" | "content_type">; large?: boolean }) {
  const kind = kindOf(entry);
  const Icon = kind.icon;
  return (
    <span data-sq="surface" className={styles.tile} data-large={large ? "" : undefined} style={{ "--tone": kind.tone } as CSSProperties} aria-hidden="true">
      <Icon size={large ? 20 : 16} strokeWidth={2} />
    </span>
  );
}
