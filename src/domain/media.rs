//! Renderer classification for stored files.
//!
//! Briefcase stores every file type. The product contract additionally tells
//! the client which renderer to open for a file, and to say "unsupported file
//! type" instead of guessing when no renderer fits. Classification is derived
//! from the file extension first, because a client-declared media type is the
//! less reliable of the two signals, and falls back to the media type only
//! when the extension is unknown.

use serde::{Deserialize, Serialize};

/// The renderer a client should use for a file.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderKind {
    /// Still image opened in a detail view.
    Image,
    /// Moving image opened in a video player.
    Video,
    /// Paginated or prose document rendered in place.
    Document,
    /// Tabular data rendered as a sheet.
    Spreadsheet,
    /// Slide deck rendered as a presentation.
    Presentation,
    /// Sound opened in an audio player.
    Audio,
    /// Container listed without extraction.
    Archive,
    /// Source or structured data rendered with syntax highlighting.
    Code,
    /// No renderer applies; only CRUD operations are offered.
    Unsupported,
}

const IMAGE_EXTENSIONS: &[&str] = &[
    "apng", "avif", "bmp", "gif", "heic", "heif", "ico", "jfif", "jpeg", "jpg", "pjpeg", "png",
    "svg", "svgz", "tif", "tiff", "webp",
];
const VIDEO_EXTENSIONS: &[&str] = &[
    "3g2", "3gp", "avi", "flv", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "ogv", "ts", "webm",
    "wmv",
];
const DOCUMENT_EXTENSIONS: &[&str] = &[
    "doc", "docx", "epub", "md", "markdown", "odt", "pages", "pdf", "rst", "rtf", "tex", "txt",
];
const SPREADSHEET_EXTENSIONS: &[&str] = &[
    "csv", "numbers", "ods", "tsv", "xls", "xlsb", "xlsm", "xlsx",
];
const PRESENTATION_EXTENSIONS: &[&str] = &["key", "odp", "pot", "potx", "ppt", "pptx"];
const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "aiff", "amr", "flac", "m4a", "mid", "midi", "mp3", "oga", "ogg", "opus", "wav", "wma",
];
const ARCHIVE_EXTENSIONS: &[&str] = &[
    "7z", "br", "bz2", "cab", "dmg", "gz", "gzip", "iso", "jar", "rar", "tar", "tgz", "txz", "xz",
    "zip", "zst",
];
const CODE_EXTENSIONS: &[&str] = &[
    "bat",
    "c",
    "cc",
    "cfg",
    "clj",
    "conf",
    "cpp",
    "cs",
    "css",
    "dart",
    "diff",
    "ejs",
    "elm",
    "env",
    "erl",
    "ex",
    "exs",
    "go",
    "gradle",
    "graphql",
    "h",
    "hbs",
    "hpp",
    "hs",
    "htm",
    "html",
    "ini",
    "ipynb",
    "java",
    "js",
    "json",
    "jsonl",
    "jsx",
    "kt",
    "kts",
    "less",
    "lock",
    "log",
    "lua",
    "m",
    "mjs",
    "ml",
    "patch",
    "php",
    "pl",
    "properties",
    "proto",
    "ps1",
    "py",
    "r",
    "rb",
    "rs",
    "sass",
    "scala",
    "scss",
    "sh",
    "sql",
    "svelte",
    "swift",
    "tf",
    "toml",
    "ts",
    "tsx",
    "vue",
    "xml",
    "yaml",
    "yml",
    "zsh",
];

/// Every renderer that a file can actually open with.
pub const ALL_RENDER_KINDS: [RenderKind; 8] = [
    RenderKind::Image,
    RenderKind::Video,
    RenderKind::Document,
    RenderKind::Spreadsheet,
    RenderKind::Presentation,
    RenderKind::Audio,
    RenderKind::Archive,
    RenderKind::Code,
];

impl RenderKind {
    /// Returns the extensions that select this renderer.
    ///
    /// Persistence compiles these lists into filter queries so that `is:image`
    /// in a filter and the `render` field on a response can never disagree.
    #[must_use]
    pub const fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Image => IMAGE_EXTENSIONS,
            Self::Video => VIDEO_EXTENSIONS,
            Self::Document => DOCUMENT_EXTENSIONS,
            Self::Spreadsheet => SPREADSHEET_EXTENSIONS,
            Self::Presentation => PRESENTATION_EXTENSIONS,
            Self::Audio => AUDIO_EXTENSIONS,
            Self::Archive => ARCHIVE_EXTENSIONS,
            Self::Code => CODE_EXTENSIONS,
            Self::Unsupported => &[],
        }
    }

    /// Returns the media-type prefixes that select this renderer.
    ///
    /// A prefix is only consulted when the file name carries no known
    /// extension, mirroring [`RenderKind::classify`].
    #[must_use]
    pub const fn media_type_prefixes(self) -> &'static [&'static str] {
        match self {
            Self::Image => &["image/"],
            Self::Video => &["video/"],
            Self::Audio => &["audio/"],
            Self::Document => &[
                "text/plain",
                "text/markdown",
                "text/richtext",
                "application/pdf",
                "application/msword",
                "application/vnd.openxmlformats-officedocument.wordprocessing",
            ],
            Self::Spreadsheet => &[
                "text/csv",
                "text/tab-separated-values",
                "application/vnd.ms-excel",
                "application/vnd.openxmlformats-officedocument.spreadsheet",
                "application/vnd.oasis.opendocument.spreadsheet",
            ],
            Self::Presentation => &[
                "application/vnd.ms-powerpoint",
                "application/vnd.openxmlformats-officedocument.presentation",
                "application/vnd.oasis.opendocument.presentation",
            ],
            Self::Archive => &[
                "application/zip",
                "application/gzip",
                "application/x-tar",
                "application/x-7z-compressed",
                "application/x-bzip2",
                "application/x-rar",
                "application/vnd.rar",
            ],
            Self::Code => &[
                "text/",
                "application/json",
                "application/xml",
                "application/javascript",
                "application/sql",
                "application/yaml",
                "application/x-yaml",
            ],
            Self::Unsupported => &[],
        }
    }

    /// Classifies a file from its name and current media type.
    #[must_use]
    pub fn classify(name: &str, content_type: Option<&str>) -> Self {
        if let Some(kind) = extension(name).and_then(|value| Self::from_extension(&value)) {
            return kind;
        }
        content_type
            .and_then(Self::from_media_type)
            .unwrap_or(Self::Unsupported)
    }

    /// Returns whether a client renderer exists for this classification.
    #[must_use]
    pub const fn is_renderable(self) -> bool {
        !matches!(self, Self::Unsupported)
    }

    fn from_extension(extension: &str) -> Option<Self> {
        ALL_RENDER_KINDS
            .into_iter()
            .find(|kind| kind.extensions().contains(&extension))
    }

    fn from_media_type(content_type: &str) -> Option<Self> {
        let essence = content_type
            .split(';')
            .next()
            .unwrap_or(content_type)
            .trim()
            .to_ascii_lowercase();
        let (top_level, subtype) = essence.split_once('/')?;
        match top_level {
            "image" => Some(Self::Image),
            "video" => Some(Self::Video),
            "audio" => Some(Self::Audio),
            "text" => Some(match subtype {
                "csv" | "tab-separated-values" => Self::Spreadsheet,
                "markdown" | "plain" | "richtext" => Self::Document,
                _ => Self::Code,
            }),
            "application" => Self::from_application_subtype(subtype),
            _ => None,
        }
    }

    fn from_application_subtype(subtype: &str) -> Option<Self> {
        if subtype.contains("spreadsheet") || subtype.ends_with("ms-excel") {
            return Some(Self::Spreadsheet);
        }
        if subtype.contains("presentation") || subtype.ends_with("ms-powerpoint") {
            return Some(Self::Presentation);
        }
        if subtype.contains("wordprocessing") || subtype.ends_with("msword") || subtype == "pdf" {
            return Some(Self::Document);
        }
        if subtype.contains("zip")
            || subtype.contains("tar")
            || subtype.contains("compressed")
            || matches!(subtype, "gzip" | "x-7z-compressed" | "x-bzip2" | "x-rar")
        {
            return Some(Self::Archive);
        }
        if subtype == "json"
            || subtype == "xml"
            || subtype.ends_with("+json")
            || subtype.ends_with("+xml")
            || subtype == "javascript"
            || subtype == "x-yaml"
            || subtype == "yaml"
            || subtype == "sql"
        {
            return Some(Self::Code);
        }
        None
    }
}

fn extension(name: &str) -> Option<String> {
    let (_, extension) = name.rsplit_once('.')?;
    if extension.is_empty() || extension.len() > 16 {
        return None;
    }
    Some(extension.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::RenderKind;

    #[test]
    fn extensions_select_the_contracted_renderer() {
        let cases = [
            ("photo.HEIC", RenderKind::Image),
            ("clip.mkv", RenderKind::Video),
            ("policy.pdf", RenderKind::Document),
            ("notes.md", RenderKind::Document),
            ("nums.csv", RenderKind::Spreadsheet),
            ("deck.key", RenderKind::Presentation),
            ("call.opus", RenderKind::Audio),
            ("backup.tgz", RenderKind::Archive),
            ("main.rs", RenderKind::Code),
            ("firmware.bin", RenderKind::Unsupported),
        ];
        for (name, expected) in cases {
            assert_eq!(RenderKind::classify(name, None), expected, "{name}");
        }
    }

    #[test]
    fn media_type_is_the_fallback_signal() {
        assert_eq!(
            RenderKind::classify("scan", Some("image/png")),
            RenderKind::Image
        );
        assert_eq!(
            RenderKind::classify("sheet", Some("application/vnd.ms-excel")),
            RenderKind::Spreadsheet
        );
        assert_eq!(
            RenderKind::classify("payload", Some("application/ld+json")),
            RenderKind::Code
        );
        assert_eq!(
            RenderKind::classify("blob", Some("application/octet-stream")),
            RenderKind::Unsupported
        );
    }

    #[test]
    fn a_known_extension_outranks_a_generic_media_type() {
        assert_eq!(
            RenderKind::classify("report.pdf", Some("application/octet-stream")),
            RenderKind::Document
        );
        assert!(RenderKind::classify("report.pdf", None).is_renderable());
        assert!(!RenderKind::classify("report", None).is_renderable());
    }
}
