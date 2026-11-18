//! Capture history: an index of past screenshots, recordings and `.lumora`
//! documents, browsed in the history window (⌥⇧H).
//!
//! The index is a cache, never the source of truth. It is rebuilt by
//! scanning the save folder ([`scan_folder`]) and merged with
//! `~/Library/Application Support/Lumora/history.json` ([`HistoryIndex`]),
//! which remembers what a scan can't cheaply recompute: dimensions, captures
//! saved under an older file-name template or another folder, and OCR text
//! recognised on this Mac (for search). Losing it loses nothing but that
//! cache, so a corrupt or newer-schema file is moved aside and rebuilt.
//!
//! Which files count as Lumora's: every `.lumora` document; a PNG with a
//! `.lumora` sibling or whose name matches the file-name template; a movie
//! with a Lumora sidecar (click log, steps, transcript) or a matching name.
//! Other images on the Desktop are never listed.
//!
//! Deleting from history only ever moves files to the Trash; this module
//! just says which files belong to an item ([`HistoryItem::files`]) and which
//! items a retention setting would remove ([`expired`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::i18n::Msg;

/// Current `history.json` schema.
pub const SCHEMA_VERSION: u32 = 1;
/// The index file inside the support folder.
pub const INDEX_FILE_NAME: &str = "history.json";
/// Cached thumbnails inside the support folder.
pub const THUMBNAIL_DIR_NAME: &str = "Thumbnails";
/// Longest side of a cached thumbnail, in pixels.
pub const THUMB_MAX_PX: u32 = 480;
/// OCR text kept per item (characters); longer text is cut.
pub const OCR_TEXT_MAX_CHARS: usize = 20_000;
/// Seconds per day.
const DAY: u64 = 24 * 60 * 60;

/// `~/Library/Application Support/Lumora`.
pub fn support_dir(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Lumora")
}

/// `~/Library/Application Support/Lumora/history.json`.
pub fn index_path(home: &Path) -> PathBuf {
    support_dir(home).join(INDEX_FILE_NAME)
}

/// `~/Library/Application Support/Lumora/Thumbnails`.
pub fn thumbnail_dir(home: &Path) -> PathBuf {
    support_dir(home).join(THUMBNAIL_DIR_NAME)
}

// -- items ----------------------------------------------------------------------

/// What a history item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// A PNG (maybe with an editable `.lumora` beside it).
    Screenshot,
    /// An MP4 / MOV recording.
    Recording,
    /// A `.lumora` document whose PNG is gone.
    Document,
}

/// One past capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryItem {
    /// The primary file: the PNG, the movie, or an orphaned `.lumora`.
    pub path: PathBuf,
    pub kind: ItemKind,
    /// The editable `.lumora` next to a screenshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<PathBuf>,
    /// Unix seconds (UTC).
    pub created_at: u64,
    /// Unix seconds of the primary file's last change (detects edits).
    #[serde(default)]
    pub modified_at: u64,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_px: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_px: Option<u32>,
    /// Text recognised on this Mac; `Some("")` = looked, found none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr_text: Option<String>,
}

impl HistoryItem {
    /// A bare item for `path` of `kind` with `meta`.
    pub fn new(path: PathBuf, kind: ItemKind, meta: FileMeta) -> Self {
        HistoryItem {
            path,
            kind,
            document: None,
            created_at: meta.created,
            modified_at: meta.modified,
            size_bytes: meta.len,
            width_px: None,
            height_px: None,
            ocr_text: None,
        }
    }

    /// The file name shown under the thumbnail.
    pub fn file_name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// Pixel dimensions, when known.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        Some((self.width_px?, self.height_px?))
    }

    /// The `.lumora` to reopen in the editor (preferred over the PNG).
    pub fn editable_document(&self) -> Option<&Path> {
        match self.kind {
            ItemKind::Document => Some(&self.path),
            _ => self.document.as_deref(),
        }
    }

    /// Whether the editor can open it at all (screenshots and documents).
    pub fn is_editable(&self) -> bool {
        self.kind != ItemKind::Recording
    }

    /// Every path that belongs to this item, primary file first: the
    /// `.lumora` sibling, and a recording's click log, tutorial steps,
    /// tutorial images and transcript. Callers check which exist.
    pub fn files(&self) -> Vec<PathBuf> {
        let mut out = vec![self.path.clone()];
        out.extend(self.document.iter().cloned());
        if self.kind == ItemKind::Recording {
            out.extend(recording_sidecars(&self.path));
        }
        out
    }

    /// Whether the item passes `filter` and contains every term of `query`
    /// (case-insensitive) in its file name or recognised text.
    pub fn matches(&self, filter: Filter, query: &str) -> bool {
        if !filter.accepts(self) {
            return false;
        }
        let terms = query_terms(query);
        if terms.is_empty() {
            return true;
        }
        let name = self.file_name().to_lowercase();
        let text = self.ocr_text.as_deref().unwrap_or("").to_lowercase();
        terms.iter().all(|t| name.contains(t.as_str()) || text.contains(t.as_str()))
    }

    /// Thumbnail cache key: changes whenever the file does.
    pub fn thumb_key(&self) -> String {
        thumb_key(&self.path, self.size_bytes, self.modified_at)
    }

    /// Whether OCR could still add searchable text (images not yet read).
    pub fn wants_ocr(&self) -> bool {
        self.kind != ItemKind::Recording && self.ocr_text.is_none()
    }
}

/// A recording's sidecars: click log, tutorial steps, tutorial images
/// folder and transcript.
pub fn recording_sidecars(movie: &Path) -> [PathBuf; 4] {
    [
        crate::click_log::sidecar_path(movie),
        crate::tutorial::sidecar_path(movie),
        crate::tutorial::image_dir(movie),
        crate::transcript::sidecar_path(movie),
    ]
}

/// Lower-cased, whitespace-separated search terms.
pub fn query_terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Thumbnail cache file stem for a file at `path` with `len` bytes, last
/// modified at `modified` (FNV-1a 64, hex).
pub fn thumb_key(path: &Path, len: u64, modified: u64) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    feed(path.to_string_lossy().as_bytes());
    feed(&[0]);
    feed(&len.to_le_bytes());
    feed(&modified.to_le_bytes());
    format!("{h:016x}")
}

/// Delete cached thumbnails (`<key>.png` in `dir`) whose key is not in
/// `live`; returns how many were removed. Only `.png` files with a 16-hex
/// stem are touched, so nothing else that ends up in the folder is lost.
pub fn prune_thumbnails(dir: &Path, live: &HashSet<String>) -> usize {
    let Ok(read) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for e in read.flatten() {
        let p = e.path();
        let is_png = p.extension().is_some_and(|x| x == "png");
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else { continue };
        let is_key = stem.len() == 16 && stem.bytes().all(|b| b.is_ascii_hexdigit());
        if is_png && is_key && !live.contains(stem) && std::fs::remove_file(&p).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The type filter above the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Screenshots,
    Recordings,
    /// Anything with an editable `.lumora`.
    Documents,
}

impl Filter {
    pub const ALL: [Filter; 4] = [Filter::All, Filter::Screenshots, Filter::Recordings, Filter::Documents];

    pub fn msg(self) -> Msg {
        match self {
            Filter::All => Msg::HistoryFilterAll,
            Filter::Screenshots => Msg::HistoryFilterScreenshots,
            Filter::Recordings => Msg::HistoryFilterRecordings,
            Filter::Documents => Msg::HistoryFilterDocuments,
        }
    }

    pub fn accepts(self, item: &HistoryItem) -> bool {
        match self {
            Filter::All => true,
            Filter::Screenshots => item.kind != ItemKind::Recording,
            Filter::Recordings => item.kind == ItemKind::Recording,
            Filter::Documents => item.editable_document().is_some(),
        }
    }
}

// -- retention --------------------------------------------------------------------

/// How long history keeps captures. Anything but `KeepAll` only *offers* to
/// move older captures to the Trash, with confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Retention {
    #[default]
    #[serde(rename = "all")]
    KeepAll,
    #[serde(rename = "30d")]
    Days30,
    #[serde(rename = "90d")]
    Days90,
}

impl Retention {
    pub const ALL: [Retention; 3] = [Retention::KeepAll, Retention::Days30, Retention::Days90];

    pub fn days(self) -> Option<u64> {
        match self {
            Retention::KeepAll => None,
            Retention::Days30 => Some(30),
            Retention::Days90 => Some(90),
        }
    }

    pub fn msg(self) -> Msg {
        match self {
            Retention::KeepAll => Msg::HistoryKeepAll,
            Retention::Days30 => Msg::HistoryKeep30,
            Retention::Days90 => Msg::HistoryKeep90,
        }
    }
}

/// `[history]` in `settings.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HistorySettings {
    pub retention: Retention,
}

/// Items older than `retention` allows at `now` (unix seconds).
pub fn expired(items: &[HistoryItem], now: u64, retention: Retention) -> Vec<&HistoryItem> {
    let Some(days) = retention.days() else { return Vec::new() };
    let max_age = days * DAY;
    items.iter().filter(|i| now.saturating_sub(i.created_at) > max_age).collect()
}

// -- the index file -------------------------------------------------------------------

#[derive(Debug, Error, PartialEq)]
pub enum IndexError {
    #[error("history index is not valid JSON: {0}")]
    Json(String),
    #[error("history index has no schema_version")]
    MissingSchemaVersion,
    #[error("history index schema {found} is newer than this Lumora ({supported})")]
    FutureSchema { found: u32, supported: u32 },
    #[error("no migration from history schema {0}")]
    NoMigration(u32),
    #[error("could not read or write the history index: {0}")]
    Io(String),
}

/// `history.json`: every known capture, newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryIndex {
    pub schema_version: u32,
    #[serde(default)]
    pub items: Vec<HistoryItem>,
}

impl Default for HistoryIndex {
    fn default() -> Self {
        HistoryIndex { schema_version: SCHEMA_VERSION, items: Vec::new() }
    }
}

type MigrationStep = fn(Value) -> Result<Value, IndexError>;

/// `MIGRATIONS[n - 1]` upgrades schema `n` to `n + 1`. Schema 1 is the first,
/// so there is nothing to run yet; the next schema adds its step here.
const MIGRATIONS: [MigrationStep; (SCHEMA_VERSION - 1) as usize] = [];

/// Upgrade an index from schema `from` to [`SCHEMA_VERSION`].
pub fn migrate(mut json: Value, from: u32) -> Result<Value, IndexError> {
    migrate_with(&mut json, from, SCHEMA_VERSION, &MIGRATIONS)?;
    Ok(json)
}

fn migrate_with(json: &mut Value, from: u32, to: u32, steps: &[MigrationStep]) -> Result<(), IndexError> {
    if from > to {
        return Err(IndexError::FutureSchema { found: from, supported: to });
    }
    if from == 0 {
        return Err(IndexError::NoMigration(0));
    }
    for v in from..to {
        let step = steps.get((v - 1) as usize).ok_or(IndexError::NoMigration(v))?;
        *json = step(std::mem::take(json))?;
    }
    if let Some(obj) = json.as_object_mut() {
        obj.insert("schema_version".into(), Value::from(to));
    }
    Ok(())
}

impl HistoryIndex {
    /// Parse (and migrate) `history.json`. Unknown keys are ignored and
    /// missing optional ones default, so additive changes need no migration.
    pub fn parse(json: &str) -> Result<HistoryIndex, IndexError> {
        let value: Value = serde_json::from_str(json).map_err(|e| IndexError::Json(e.to_string()))?;
        let from = value.get("schema_version").and_then(Value::as_u64).ok_or(IndexError::MissingSchemaVersion)?;
        let from = u32::try_from(from).unwrap_or(u32::MAX);
        let value = migrate(value, from)?;
        let mut index: HistoryIndex = serde_json::from_value(value).map_err(|e| IndexError::Json(e.to_string()))?;
        index.sort();
        Ok(index)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Load from `path`; a missing file is an empty index.
    pub fn load(path: &Path) -> Result<HistoryIndex, IndexError> {
        match std::fs::read_to_string(path) {
            Ok(s) => Self::parse(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HistoryIndex::default()),
            Err(e) => Err(IndexError::Io(e.to_string())),
        }
    }

    /// Load, or start empty. An unreadable index (corrupt, or written by a
    /// newer Lumora) is moved aside to `history.json.bak` first; the next
    /// scan rebuilds what matters.
    pub fn load_or_default(path: &Path) -> HistoryIndex {
        match Self::load(path) {
            Ok(i) => i,
            Err(IndexError::Io(_)) => HistoryIndex::default(),
            Err(_) => {
                let _ = std::fs::rename(path, path.with_extension("json.bak"));
                HistoryIndex::default()
            }
        }
    }

    /// Write atomically, creating the folder.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        crate::lumora_doc::write_atomic(path, self.to_json().as_bytes())
    }

    /// Newest first; ties by path, so the order is stable.
    pub fn sort(&mut self) {
        self.items.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| a.path.cmp(&b.path)));
    }

    pub fn get(&self, path: &Path) -> Option<&HistoryItem> {
        self.items.iter().find(|i| i.path == path)
    }

    fn get_mut(&mut self, path: &Path) -> Option<&mut HistoryItem> {
        self.items.iter_mut().find(|i| i.path == path)
    }

    /// Insert or replace the item for `item.path`, keeping cached OCR text
    /// and dimensions when the file is unchanged.
    pub fn upsert(&mut self, mut item: HistoryItem) {
        if let Some(old) = self.get_mut(&item.path) {
            carry_cache(old, &mut item);
            *old = item;
        } else {
            self.items.push(item);
        }
        self.sort();
    }

    /// Forget the item whose primary file is `path`.
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.items.len();
        self.items.retain(|i| i.path != path);
        self.items.len() != before
    }

    /// Remember text recognised on the item at `path` (or the item whose
    /// `.lumora` is `path`), cut to [`OCR_TEXT_MAX_CHARS`].
    pub fn set_ocr_text(&mut self, path: &Path, text: &str) -> bool {
        let Some(item) = self.items.iter_mut().find(|i| i.path == path || i.document.as_deref() == Some(path)) else {
            return false;
        };
        let text: String = text.trim().chars().take(OCR_TEXT_MAX_CHARS).collect();
        if item.ocr_text.as_deref() == Some(text.as_str()) {
            return false;
        }
        item.ocr_text = Some(text);
        true
    }

    pub fn set_dimensions(&mut self, path: &Path, width: u32, height: u32) -> bool {
        match self.get_mut(path) {
            Some(i) if i.dimensions() != Some((width, height)) => {
                i.width_px = Some(width);
                i.height_px = Some(height);
                true
            }
            _ => false,
        }
    }

    /// Merge a fresh scan of a folder into the index.
    ///
    /// * Scanned items replace their old entries; OCR text and dimensions
    ///   carry over while the file is unchanged (same size and mtime), and
    ///   the first-seen creation time is kept.
    /// * Old entries the scan didn't produce (another folder, an older file
    ///   name template) stay while their file exists (`stat`), refreshed;
    ///   missing ones are dropped.
    /// * An old entry whose file a scanned item now owns (an orphaned
    ///   document whose PNG reappeared) is dropped as a duplicate.
    pub fn refresh(&mut self, scanned: Vec<HistoryItem>, stat: impl Fn(&Path) -> Option<FileMeta>) {
        let mut old: HashMap<PathBuf, HistoryItem> = self.items.drain(..).map(|i| (i.path.clone(), i)).collect();
        let mut claimed: HashSet<PathBuf> = HashSet::new();
        let mut items = Vec::with_capacity(scanned.len() + old.len());
        for mut item in scanned {
            if let Some(prev) = old.remove(&item.path) {
                carry_cache(&prev, &mut item);
            }
            claimed.insert(item.path.clone());
            claimed.extend(item.document.iter().cloned());
            items.push(item);
        }
        for (path, mut item) in old {
            if claimed.contains(&path) {
                continue;
            }
            let Some(meta) = stat(&path) else { continue };
            if (meta.len, meta.modified) != (item.size_bytes, item.modified_at) {
                item.size_bytes = meta.len;
                item.modified_at = meta.modified;
                item.ocr_text = None;
                if item.kind != ItemKind::Recording {
                    item.width_px = None;
                    item.height_px = None;
                }
            }
            if item.kind == ItemKind::Screenshot {
                let doc = crate::lumora_doc::doc_path_for(&path);
                item.document = stat(&doc).map(|_| doc);
            }
            if item.document.as_ref().is_some_and(|d| claimed.contains(d)) {
                continue;
            }
            claimed.insert(path);
            items.push(item);
        }
        self.items = items;
        self.sort();
    }

    /// Items without dimensions that can be read from the file header
    /// (screenshots and documents).
    pub fn missing_dimensions(&self) -> Vec<(PathBuf, ItemKind)> {
        self.items
            .iter()
            .filter(|i| i.dimensions().is_none() && i.kind != ItemKind::Recording)
            .map(|i| (i.path.clone(), i.kind))
            .collect()
    }

    /// Paths of items that could gain OCR text, newest first, at most `n`.
    pub fn wanting_ocr(&self, n: usize) -> Vec<PathBuf> {
        self.items.iter().filter(|i| i.wants_ocr()).take(n).map(|i| i.path.clone()).collect()
    }
}

/// Keep `prev`'s cached values on `item` when it describes the same file
/// contents; always keep the first-seen creation time.
fn carry_cache(prev: &HistoryItem, item: &mut HistoryItem) {
    if prev.created_at > 0 {
        item.created_at = prev.created_at;
    }
    if (prev.size_bytes, prev.modified_at) == (item.size_bytes, item.modified_at) {
        if item.ocr_text.is_none() {
            item.ocr_text = prev.ocr_text.clone();
        }
        if item.dimensions().is_none() {
            item.width_px = prev.width_px;
            item.height_px = prev.height_px;
        }
    }
}

// -- scanning -----------------------------------------------------------------------

/// What the file system says about a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileMeta {
    pub len: u64,
    /// Unix seconds.
    pub modified: u64,
    /// Unix seconds (birth time; the modification time where unavailable).
    pub created: u64,
}

impl FileMeta {
    /// Stat a regular file (`None` for folders and missing paths).
    pub fn of(path: &Path) -> Option<FileMeta> {
        let m = std::fs::metadata(path).ok()?;
        if !m.is_file() {
            return None;
        }
        let secs = |t: std::io::Result<std::time::SystemTime>| {
            t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs())
        };
        let modified = secs(m.modified()).unwrap_or(0);
        let created = secs(m.created()).filter(|c| *c > 0).unwrap_or(modified);
        Some(FileMeta { len: m.len(), modified, created })
    }
}

/// What a file is, by extension. Hidden files are never anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileClass {
    Image,
    Movie,
    Document,
}

pub fn classify(path: &Path) -> Option<FileClass> {
    let name = path.file_name()?.to_str()?;
    if name.starts_with('.') {
        return None;
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some(FileClass::Image),
        "mp4" | "mov" | "m4v" => Some(FileClass::Movie),
        e if e == crate::lumora_doc::EXTENSION => Some(FileClass::Document),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Piece {
    Literal(String),
    Digits(usize),
    /// `Screenshot` / `Recording` (`{kind}`).
    Kind,
}

/// Recognises file stems produced by a file-name template (see
/// [`crate::filename`]), with or without a ` (2)` uniqueness suffix.
#[derive(Debug, Clone, PartialEq)]
pub struct NameMatcher {
    pieces: Vec<Piece>,
}

impl NameMatcher {
    pub fn new(template: &str) -> NameMatcher {
        let template = if template.trim().is_empty() { crate::filename::DEFAULT_TEMPLATE } else { template };
        let mut pieces = Vec::new();
        let mut lit = String::new();
        let mut rest = template;
        let d = Piece::Digits;
        let l = |s: &str| Piece::Literal(s.to_string());
        let tokens: [(&str, Vec<Piece>); 9] = [
            // `YYYY-MM-DD` and `hh.mm.ss`, as `filename::render` writes them.
            ("{date}", vec![d(4), l("-"), d(2), l("-"), d(2)]),
            ("{time}", vec![d(2), l("."), d(2), l("."), d(2)]),
            ("{YYYY}", vec![d(4)]),
            ("{MM}", vec![d(2)]),
            ("{DD}", vec![d(2)]),
            ("{hh}", vec![d(2)]),
            ("{mm}", vec![d(2)]),
            ("{ss}", vec![d(2)]),
            ("{kind}", vec![Piece::Kind]),
        ];
        'outer: while !rest.is_empty() {
            for (tok, expansion) in &tokens {
                if let Some(r) = rest.strip_prefix(tok) {
                    if !lit.is_empty() {
                        pieces.push(Piece::Literal(sanitise(&std::mem::take(&mut lit))));
                    }
                    pieces.extend(expansion.iter().cloned());
                    rest = r;
                    continue 'outer;
                }
            }
            let c = rest.chars().next().unwrap_or_default();
            lit.push(c);
            rest = &rest[c.len_utf8()..];
        }
        if !lit.is_empty() {
            pieces.push(Piece::Literal(sanitise(&lit)));
        }
        // `render` trims the result; trim the outer literals the same way.
        if let Some(Piece::Literal(s)) = pieces.first_mut() {
            *s = s.trim_start().trim_start_matches('.').trim_start().to_string();
        }
        if let Some(Piece::Literal(s)) = pieces.last_mut() {
            *s = s.trim_end().to_string();
        }
        pieces.retain(|p| !matches!(p, Piece::Literal(s) if s.is_empty()));
        NameMatcher { pieces }
    }

    /// Whether `stem` (file name without extension) was made by the
    /// template, allowing a trailing ` (N)`.
    pub fn matches(&self, stem: &str) -> bool {
        if self.pieces.is_empty() {
            return false;
        }
        let base = strip_unique_suffix(stem);
        match_pieces(&self.pieces, base) || (base != stem && match_pieces(&self.pieces, stem))
    }
}

/// `"a (2)"` → `"a"`; anything else unchanged.
fn strip_unique_suffix(stem: &str) -> &str {
    if let Some(open) = stem.rfind(" (") {
        let inner = &stem[open + 2..];
        if let Some(n) = inner.strip_suffix(')') {
            if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
                return &stem[..open];
            }
        }
    }
    stem
}

/// The same character clean-up [`crate::filename::render`] applies.
fn sanitise(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).map(|c| if c == '/' || c == ':' { '-' } else { c }).collect()
}

fn match_pieces(pieces: &[Piece], s: &str) -> bool {
    let Some((first, rest)) = pieces.split_first() else { return s.is_empty() };
    match first {
        Piece::Literal(l) => s.strip_prefix(l.as_str()).is_some_and(|r| match_pieces(rest, r)),
        Piece::Digits(n) => {
            s.len() >= *n
                && s.is_char_boundary(*n)
                && s[..*n].bytes().all(|b| b.is_ascii_digit())
                && match_pieces(rest, &s[*n..])
        }
        Piece::Kind => {
            ["Screenshot", "Recording"].iter().any(|k| s.strip_prefix(k).is_some_and(|r| match_pieces(rest, r)))
        }
    }
}

/// Pixel size from the start of a PNG file (signature + IHDR).
pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];
    if bytes.len() < 24 || bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// Backing scale for an image file's DPI: `dpi / 72`, rounded to a whole
/// scale in 1…4 (Lumora writes 144 dpi for a 2× capture); 1× without DPI.
pub fn scale_for_dpi(dpi: Option<f64>) -> f64 {
    match dpi {
        Some(d) if d.is_finite() && d > 0.0 => (d / 72.0).round().clamp(1.0, 4.0),
        _ => 1.0,
    }
}

/// Read a screenshot's or document's pixel size from its file header.
pub fn read_dimensions(path: &Path, kind: ItemKind) -> Option<(u32, u32)> {
    use std::io::Read;
    match kind {
        ItemKind::Screenshot => {
            let mut buf = [0u8; 24];
            std::fs::File::open(path).ok()?.read_exact(&mut buf).ok()?;
            png_dimensions(&buf)
        }
        ItemKind::Document => {
            let h = crate::lumora_doc::read_header(path).ok()?;
            Some((h.image.width_px, h.image.height_px))
        }
        ItemKind::Recording => None,
    }
}

/// List Lumora's captures in `dir` (not recursive). Dimensions and OCR text
/// are left for [`HistoryIndex::refresh`] to carry over and the caller to
/// fill.
pub fn scan_folder(dir: &Path, names: &NameMatcher) -> Vec<HistoryItem> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let files: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
    let present: HashSet<&Path> = files.iter().map(PathBuf::as_path).collect();
    let has = |p: &Path| present.contains(p);
    let stem_of = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut out = Vec::new();
    for path in &files {
        let Some(class) = classify(path) else { continue };
        let item = match class {
            FileClass::Image => {
                let doc = crate::lumora_doc::doc_path_for(path);
                let has_doc = has(&doc);
                if !has_doc && !names.matches(&stem_of(path)) {
                    continue;
                }
                FileMeta::of(path).map(|m| HistoryItem {
                    document: has_doc.then_some(doc),
                    ..HistoryItem::new(path.clone(), ItemKind::Screenshot, m)
                })
            }
            FileClass::Movie => {
                let sidecar = recording_sidecars(path).iter().any(|s| has(s));
                if !sidecar && !names.matches(&stem_of(path)) {
                    continue;
                }
                FileMeta::of(path).map(|m| HistoryItem::new(path.clone(), ItemKind::Recording, m))
            }
            FileClass::Document => {
                // Listed with its PNG when that exists.
                if files
                    .iter()
                    .any(|f| classify(f) == Some(FileClass::Image) && crate::lumora_doc::doc_path_for(f) == *path)
                {
                    continue;
                }
                FileMeta::of(path).map(|m| HistoryItem::new(path.clone(), ItemKind::Document, m))
            }
        };
        out.extend(item);
    }
    out
}

// -- grid layout ---------------------------------------------------------------------

/// Thumbnail grid geometry for the history window (points, y down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub columns: usize,
    pub cell_width: f64,
    pub cell_height: f64,
    pub gap: f64,
    pub margin: f64,
}

/// Narrowest a cell may get before a column is dropped.
pub const CELL_MIN_W: f64 = 180.0;
/// Text + buttons under the thumbnail.
pub const CELL_FOOTER_H: f64 = 72.0;

impl Grid {
    /// As many columns of at least [`CELL_MIN_W`] as fit `width`, sharing
    /// the spare width; thumbnails are 16:10.
    pub fn new(width: f64) -> Grid {
        let (gap, margin) = (16.0, 20.0);
        let usable = (width - 2.0 * margin).max(CELL_MIN_W);
        let columns = (((usable + gap) / (CELL_MIN_W + gap)).floor() as usize).max(1);
        let cell_width = (usable - gap * (columns as f64 - 1.0)) / columns as f64;
        Grid { columns, cell_width, cell_height: cell_width * 0.625 + CELL_FOOTER_H, gap, margin }
    }

    /// Height of the thumbnail part of a cell.
    pub fn thumb_height(&self) -> f64 {
        self.cell_height - CELL_FOOTER_H
    }

    /// Top-left corner of cell `index`.
    pub fn origin(&self, index: usize) -> (f64, f64) {
        let (row, col) = (index / self.columns, index % self.columns);
        (
            self.margin + col as f64 * (self.cell_width + self.gap),
            self.margin + row as f64 * (self.cell_height + self.gap),
        )
    }

    /// Document height for `count` cells (plus `extra` below them).
    pub fn content_height(&self, count: usize, extra: f64) -> f64 {
        let rows = count.div_ceil(self.columns);
        let cells = if rows == 0 { 0.0 } else { rows as f64 * self.cell_height + (rows as f64 - 1.0) * self.gap };
        2.0 * self.margin + cells + extra
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(len: u64, t: u64) -> FileMeta {
        FileMeta { len, modified: t, created: t }
    }

    fn item(path: &str, kind: ItemKind, t: u64) -> HistoryItem {
        HistoryItem::new(PathBuf::from(path), kind, meta(100, t))
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lumora-history-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn paths_live_in_application_support() {
        let h = Path::new("/Users/a");
        assert_eq!(index_path(h), PathBuf::from("/Users/a/Library/Application Support/Lumora/history.json"));
        assert_eq!(thumbnail_dir(h), PathBuf::from("/Users/a/Library/Application Support/Lumora/Thumbnails"));
    }

    #[test]
    fn default_template_names_match() {
        let m = NameMatcher::new(crate::filename::DEFAULT_TEMPLATE);
        assert!(m.matches("Lumora 2026-11-15 at 04.30.12"));
        assert!(m.matches("Lumora 2026-11-15 at 04.30.12 (2)"));
        assert!(!m.matches("Lumora 2026-11-15 at 04.30"));
        assert!(!m.matches("Screenshot 2026-11-15 at 04.30.12"));
        assert!(!m.matches("Lumora 2026-11-15 at 04.30.12 copy"));
        assert!(!m.matches("Lumora 2026-1x-15 at 04.30.12"));
    }

    #[test]
    fn custom_templates_match_what_render_produces() {
        use crate::filename::{render, CaptureKind, DateTimeParts};
        let t = DateTimeParts { year: 2026, month: 1, day: 2, hour: 3, minute: 4, second: 5 };
        for tpl in ["{kind}_{YYYY}{MM}{DD}-{hh}{mm}{ss}", "截图 {date} {time}", "  shot:{date}  ", "{kind}"] {
            let m = NameMatcher::new(tpl);
            for kind in [CaptureKind::Screenshot, CaptureKind::Recording] {
                let name = render(tpl, &t, kind);
                assert!(m.matches(&name), "{tpl:?} should match {name:?}");
                assert!(m.matches(&format!("{name} (12)")), "{tpl:?} with suffix");
            }
        }
        assert!(!NameMatcher::new("{kind}").matches("Video"));
        // A constant template is matched literally.
        assert!(NameMatcher::new("capture").matches("capture (3)"));
        assert!(!NameMatcher::new("capture").matches("capture 3"));
        // Empty falls back to the default.
        assert!(NameMatcher::new("  ").matches("Lumora 2026-11-15 at 04.30.12"));
    }

    #[test]
    fn png_header_dimensions() {
        let mut b = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n', 0, 0, 0, 13];
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&3024u32.to_be_bytes());
        b.extend_from_slice(&1964u32.to_be_bytes());
        assert_eq!(png_dimensions(&b), Some((3024, 1964)));
        assert_eq!(png_dimensions(&b[..20]), None);
        let mut bad = b.clone();
        bad[0] = 0;
        assert_eq!(png_dimensions(&bad), None);
        let mut zero = b;
        zero[16..20].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(png_dimensions(&zero), None);
    }

    #[test]
    fn dpi_maps_to_backing_scale() {
        assert_eq!(scale_for_dpi(Some(144.0)), 2.0);
        assert_eq!(scale_for_dpi(Some(72.0)), 1.0);
        assert_eq!(scale_for_dpi(Some(216.0)), 3.0);
        assert_eq!(scale_for_dpi(Some(150.0)), 2.0);
        assert_eq!(scale_for_dpi(Some(10_000.0)), 4.0);
        assert_eq!(scale_for_dpi(Some(0.0)), 1.0);
        assert_eq!(scale_for_dpi(Some(f64::NAN)), 1.0);
        assert_eq!(scale_for_dpi(None), 1.0);
    }

    #[test]
    fn classify_by_extension_ignoring_hidden() {
        assert_eq!(classify(Path::new("/a/x.PNG")), Some(FileClass::Image));
        assert_eq!(classify(Path::new("/a/x.mov")), Some(FileClass::Movie));
        assert_eq!(classify(Path::new("/a/x.mp4")), Some(FileClass::Movie));
        assert_eq!(classify(Path::new("/a/x.lumora")), Some(FileClass::Document));
        assert_eq!(classify(Path::new("/a/.x.lumora.123.tmp")), None);
        assert_eq!(classify(Path::new("/a/.hidden.png")), None);
        assert_eq!(classify(Path::new("/a/x.jpg")), None);
        assert_eq!(classify(Path::new("/a/x")), None);
    }

    #[test]
    fn scan_adopts_only_lumora_files() {
        let d = temp_dir("scan");
        let w = |name: &str| std::fs::write(d.join(name), b"x").unwrap();
        w("Lumora 2026-11-15 at 04.30.12.png");
        w("Lumora 2026-11-15 at 04.30.12.lumora");
        w("Lumora 2026-11-15 at 05.00.00.png");
        w("holiday.png"); // someone else's image
        w("renamed.png");
        w("renamed.lumora"); // editable → ours
        w("orphan.lumora"); // PNG deleted
        w("Lumora 2026-11-15 at 06.00.00.mp4");
        w("demo.mov");
        w("demo.events.json"); // click log → ours
        w("other.mp4");
        w(".Lumora 2026-11-15 at 07.00.00.png");
        std::fs::create_dir_all(d.join("Lumora 2026-11-15 at 08.00.00.png")).unwrap(); // a folder
        let m = NameMatcher::new(crate::filename::DEFAULT_TEMPLATE);
        let mut items = scan_folder(&d, &m);
        items.sort_by(|a, b| a.path.cmp(&b.path));
        let names: Vec<(String, ItemKind, bool)> =
            items.iter().map(|i| (i.file_name(), i.kind, i.document.is_some())).collect();
        assert_eq!(
            names,
            vec![
                ("Lumora 2026-11-15 at 04.30.12.png".into(), ItemKind::Screenshot, true),
                ("Lumora 2026-11-15 at 05.00.00.png".into(), ItemKind::Screenshot, false),
                ("Lumora 2026-11-15 at 06.00.00.mp4".into(), ItemKind::Recording, false),
                ("demo.mov".into(), ItemKind::Recording, false),
                ("orphan.lumora".into(), ItemKind::Document, false),
                ("renamed.png".into(), ItemKind::Screenshot, true),
            ]
        );
        assert!(items.iter().all(|i| i.size_bytes == 1 && i.created_at > 0));
        assert!(scan_folder(&d.join("missing"), &m).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn dimensions_from_png_and_document_headers() {
        let d = temp_dir("dims");
        let mut png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n', 0, 0, 0, 13];
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&800u32.to_be_bytes());
        png.extend_from_slice(&600u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        std::fs::write(d.join("a.png"), &png).unwrap();
        assert_eq!(read_dimensions(&d.join("a.png"), ItemKind::Screenshot), Some((800, 600)));
        let doc = crate::lumora_doc::LumoraDoc::from_editor_state(
            &crate::annotate::Document::new(crate::geometry::Size::new(400.0, 300.0)),
            png.clone(),
            crate::lumora_doc::ImageInfo { width_px: 800, height_px: 600, scale: 2.0 },
            None,
            1,
        );
        std::fs::write(d.join("a.lumora"), doc.encode()).unwrap();
        assert_eq!(read_dimensions(&d.join("a.lumora"), ItemKind::Document), Some((800, 600)));
        assert_eq!(read_dimensions(&d.join("a.lumora"), ItemKind::Recording), None);
        assert_eq!(read_dimensions(&d.join("nope.png"), ItemKind::Screenshot), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn filter_and_search() {
        let mut shot = item("/c/Lumora 2026-11-15 at 04.30.12.png", ItemKind::Screenshot, 10);
        shot.ocr_text = Some("Invoice total 42 €\n发票号码 123".into());
        let mut edited = item("/c/plan.png", ItemKind::Screenshot, 11);
        edited.document = Some(PathBuf::from("/c/plan.lumora"));
        let movie = item("/c/Demo Recording.mp4", ItemKind::Recording, 12);
        let orphan = item("/c/old.lumora", ItemKind::Document, 13);
        assert!(shot.matches(Filter::All, ""));
        assert!(shot.matches(Filter::Screenshots, "invoice"));
        assert!(shot.matches(Filter::All, "INVOICE 42"));
        assert!(shot.matches(Filter::All, "发票"));
        assert!(shot.matches(Filter::All, "lumora 2026"));
        assert!(!shot.matches(Filter::All, "invoice receipt"), "every term must match");
        assert!(!shot.matches(Filter::Recordings, ""));
        assert!(!shot.matches(Filter::Documents, ""));
        assert!(edited.matches(Filter::Documents, "plan"));
        assert!(orphan.matches(Filter::Documents, ""));
        assert!(orphan.matches(Filter::Screenshots, ""));
        assert!(movie.matches(Filter::Recordings, "demo"));
        assert!(!movie.matches(Filter::Screenshots, ""));
        assert_eq!(edited.editable_document(), Some(Path::new("/c/plan.lumora")));
        assert_eq!(orphan.editable_document(), Some(Path::new("/c/old.lumora")));
        assert_eq!(shot.editable_document(), None);
        assert!(!movie.is_editable() && orphan.is_editable());
        assert!(edited.wants_ocr() && !shot.wants_ocr() && !movie.wants_ocr());
    }

    #[test]
    fn files_include_document_and_recording_sidecars() {
        let mut shot = item("/c/a.png", ItemKind::Screenshot, 1);
        shot.document = Some(PathBuf::from("/c/a.lumora"));
        assert_eq!(shot.files(), vec![PathBuf::from("/c/a.png"), PathBuf::from("/c/a.lumora")]);
        let movie = item("/c/m.mp4", ItemKind::Recording, 1);
        assert_eq!(
            movie.files(),
            vec![
                PathBuf::from("/c/m.mp4"),
                PathBuf::from("/c/m.events.json"),
                PathBuf::from("/c/m.steps.json"),
                PathBuf::from("/c/m.tutorial"),
                PathBuf::from("/c/m.transcript.json"),
            ]
        );
    }

    #[test]
    fn retention_selects_only_older_items() {
        let now = 1_000 * DAY;
        let items = vec![
            item("/a", ItemKind::Screenshot, now - 31 * DAY),
            item("/b", ItemKind::Screenshot, now - 30 * DAY),
            item("/c", ItemKind::Recording, now - 91 * DAY),
            item("/d", ItemKind::Screenshot, now + 5), // clock skew: never expired
        ];
        assert!(expired(&items, now, Retention::KeepAll).is_empty());
        let e30: Vec<&Path> = expired(&items, now, Retention::Days30).iter().map(|i| i.path.as_path()).collect();
        assert_eq!(e30, vec![Path::new("/a"), Path::new("/c")]);
        let e90: Vec<&Path> = expired(&items, now, Retention::Days90).iter().map(|i| i.path.as_path()).collect();
        assert_eq!(e90, vec![Path::new("/c")]);
    }

    #[test]
    fn retention_settings_serde() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct W {
            history: HistorySettings,
        }
        let w = W { history: HistorySettings { retention: Retention::Days90 } };
        let t = toml::to_string(&w).unwrap();
        assert!(t.contains("retention = \"90d\""), "{t}");
        assert_eq!(toml::from_str::<W>(&t).unwrap(), w);
        assert_eq!(toml::from_str::<HistorySettings>("").unwrap().retention, Retention::KeepAll);
        assert!(toml::from_str::<HistorySettings>("retention = \"7d\"").is_err());
    }

    #[test]
    fn index_round_trips_and_tolerates_unknown_and_missing_keys() {
        let mut i = HistoryIndex::default();
        let mut a = item("/c/a.png", ItemKind::Screenshot, 5);
        a.width_px = Some(10);
        a.height_px = Some(20);
        a.ocr_text = Some("hello".into());
        i.upsert(a);
        i.upsert(item("/c/b.mp4", ItemKind::Recording, 9));
        let json = i.to_json();
        assert!(json.contains("\"schema_version\": 1") && json.contains("\"kind\": \"recording\""), "{json}");
        assert_eq!(HistoryIndex::parse(&json).unwrap(), i);
        assert_eq!(i.items[0].path, PathBuf::from("/c/b.mp4"), "newest first");
        let minimal = r#"{"schema_version":1,"future":true,"items":[{"path":"/x.png","kind":"screenshot","created_at":3,"extra":1}]}"#;
        let p = HistoryIndex::parse(minimal).unwrap();
        assert_eq!(p.items[0].size_bytes, 0);
        assert_eq!(p.items[0].ocr_text, None);
        assert_eq!(HistoryIndex::parse(r#"{"schema_version":1}"#).unwrap(), HistoryIndex::default());
    }

    #[test]
    fn index_schema_errors_and_migration_steps() {
        assert_eq!(HistoryIndex::parse(r#"{"items":[]}"#), Err(IndexError::MissingSchemaVersion));
        assert_eq!(
            HistoryIndex::parse(r#"{"schema_version":7,"items":[]}"#),
            Err(IndexError::FutureSchema { found: 7, supported: SCHEMA_VERSION })
        );
        assert_eq!(HistoryIndex::parse(r#"{"schema_version":0}"#), Err(IndexError::NoMigration(0)));
        assert!(matches!(HistoryIndex::parse("{"), Err(IndexError::Json(_))));
        assert!(matches!(
            HistoryIndex::parse(r#"{"schema_version":1,"items":[{"path":"/x"}]}"#),
            Err(IndexError::Json(_))
        ));
        // The step machinery: a hypothetical schema 2 renaming `items` → `entries`
        // upgrades a schema-1 file and stamps the new version.
        fn v1_to_v2(mut v: Value) -> Result<Value, IndexError> {
            if let Some(o) = v.as_object_mut() {
                let items = o.remove("items").unwrap_or(Value::Array(vec![]));
                o.insert("entries".into(), items);
            }
            Ok(v)
        }
        let mut v: Value = serde_json::from_str(r#"{"schema_version":1,"items":[1]}"#).unwrap();
        migrate_with(&mut v, 1, 2, &[v1_to_v2]).unwrap();
        assert_eq!(v, serde_json::json!({"schema_version": 2, "entries": [1]}));
        let mut v: Value = serde_json::json!({"schema_version": 1});
        assert_eq!(migrate_with(&mut v, 1, 3, &[v1_to_v2]), Err(IndexError::NoMigration(2)));
    }

    #[test]
    fn index_on_disk_backs_up_unreadable_files() {
        let d = temp_dir("disk");
        let path = d.join("Lumora").join(INDEX_FILE_NAME);
        assert_eq!(HistoryIndex::load(&path).unwrap(), HistoryIndex::default());
        let mut i = HistoryIndex::default();
        i.upsert(item("/c/a.png", ItemKind::Screenshot, 5));
        i.save(&path).unwrap();
        assert_eq!(HistoryIndex::load(&path).unwrap(), i);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(HistoryIndex::load_or_default(&path), HistoryIndex::default());
        assert!(path.with_extension("json.bak").exists());
        std::fs::write(&path, r#"{"schema_version":99}"#).unwrap();
        assert_eq!(HistoryIndex::load_or_default(&path), HistoryIndex::default());
        assert_eq!(std::fs::read_to_string(path.with_extension("json.bak")).unwrap(), r#"{"schema_version":99}"#);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refresh_merges_scan_with_cached_values() {
        let mut idx = HistoryIndex::default();
        let mut a = item("/save/a.png", ItemKind::Screenshot, 100);
        a.ocr_text = Some("cached".into());
        a.width_px = Some(4);
        a.height_px = Some(3);
        let mut changed = item("/save/b.png", ItemKind::Screenshot, 100);
        changed.ocr_text = Some("stale".into());
        let elsewhere = item("/old/c.png", ItemKind::Screenshot, 50);
        let gone = item("/old/gone.png", ItemKind::Screenshot, 40);
        let orphan = item("/save/d.lumora", ItemKind::Document, 30);
        for i in [a, changed, elsewhere, gone, orphan] {
            idx.upsert(i);
        }
        // Scan: a unchanged, b edited (new size), d's PNG reappeared.
        let scanned = vec![
            HistoryItem::new(
                "/save/a.png".into(),
                ItemKind::Screenshot,
                FileMeta { len: 100, modified: 100, created: 200 },
            ),
            HistoryItem::new(
                "/save/b.png".into(),
                ItemKind::Screenshot,
                FileMeta { len: 999, modified: 150, created: 100 },
            ),
            HistoryItem {
                document: Some("/save/d.lumora".into()),
                ..HistoryItem::new("/save/d.png".into(), ItemKind::Screenshot, meta(5, 30))
            },
        ];
        idx.refresh(scanned, |p| match p.to_str() {
            Some("/old/c.png") => Some(meta(100, 50)),
            _ => None,
        });
        let paths: Vec<&str> = idx.items.iter().map(|i| i.path.to_str().unwrap()).collect();
        assert_eq!(paths, vec!["/save/a.png", "/save/b.png", "/old/c.png", "/save/d.png"]);
        let a = idx.get(Path::new("/save/a.png")).unwrap();
        assert_eq!((a.ocr_text.as_deref(), a.dimensions(), a.created_at), (Some("cached"), Some((4, 3)), 100));
        let b = idx.get(Path::new("/save/b.png")).unwrap();
        assert_eq!((b.ocr_text.as_deref(), b.size_bytes), (None, 999), "changed file loses its OCR text");
        assert!(idx.get(Path::new("/save/d.lumora")).is_none(), "orphan replaced by its PNG");
    }

    #[test]
    fn refresh_restats_entries_outside_the_scan() {
        let mut idx = HistoryIndex::default();
        let mut c = item("/old/c.png", ItemKind::Screenshot, 50);
        c.ocr_text = Some("x".into());
        c.width_px = Some(1);
        c.height_px = Some(1);
        idx.upsert(c);
        idx.refresh(Vec::new(), |p| match p.to_str() {
            Some("/old/c.png") => Some(meta(7, 60)),
            Some("/old/c.lumora") => Some(meta(1, 60)),
            _ => None,
        });
        let c = &idx.items[0];
        assert_eq!((c.size_bytes, c.modified_at, c.ocr_text.as_deref(), c.dimensions()), (7, 60, None, None));
        assert_eq!(c.document.as_deref(), Some(Path::new("/old/c.lumora")));
        assert_eq!(c.created_at, 50, "first-seen creation time kept");
    }

    #[test]
    fn ocr_text_and_dimensions_setters() {
        let mut idx = HistoryIndex::default();
        let mut a = item("/c/a.png", ItemKind::Screenshot, 1);
        a.document = Some("/c/a.lumora".into());
        idx.upsert(a);
        assert!(idx.set_ocr_text(Path::new("/c/a.lumora"), "  hi  "));
        assert_eq!(idx.items[0].ocr_text.as_deref(), Some("hi"));
        assert!(!idx.set_ocr_text(Path::new("/c/a.png"), "hi"), "unchanged");
        assert!(!idx.set_ocr_text(Path::new("/c/zzz.png"), "hi"));
        let long = "字".repeat(OCR_TEXT_MAX_CHARS + 10);
        assert!(idx.set_ocr_text(Path::new("/c/a.png"), &long));
        assert_eq!(idx.items[0].ocr_text.as_ref().unwrap().chars().count(), OCR_TEXT_MAX_CHARS);
        assert!(idx.set_dimensions(Path::new("/c/a.png"), 3, 4));
        assert!(!idx.set_dimensions(Path::new("/c/a.png"), 3, 4));
        assert_eq!(idx.missing_dimensions(), vec![]);
        assert_eq!(idx.wanting_ocr(5), Vec::<PathBuf>::new());
        idx.upsert(item("/c/b.png", ItemKind::Screenshot, 2));
        idx.upsert(item("/c/m.mp4", ItemKind::Recording, 3));
        assert_eq!(idx.missing_dimensions(), vec![(PathBuf::from("/c/b.png"), ItemKind::Screenshot)]);
        assert_eq!(idx.wanting_ocr(5), vec![PathBuf::from("/c/b.png")]);
        assert!(idx.remove(Path::new("/c/b.png")));
        assert!(!idx.remove(Path::new("/c/b.png")));
    }

    #[test]
    fn upsert_keeps_cache_for_unchanged_files() {
        let mut idx = HistoryIndex::default();
        let mut a = item("/c/a.png", ItemKind::Screenshot, 10);
        a.ocr_text = Some("t".into());
        idx.upsert(a);
        idx.upsert(HistoryItem::new(
            "/c/a.png".into(),
            ItemKind::Screenshot,
            FileMeta { len: 100, modified: 10, created: 99 },
        ));
        assert_eq!(idx.items.len(), 1);
        assert_eq!(idx.items[0].ocr_text.as_deref(), Some("t"));
        assert_eq!(idx.items[0].created_at, 10);
    }

    #[test]
    fn thumb_keys_change_with_the_file() {
        let p = Path::new("/c/a.png");
        let k = thumb_key(p, 10, 20);
        assert_eq!(k.len(), 16);
        assert_eq!(k, thumb_key(p, 10, 20));
        assert_ne!(k, thumb_key(p, 11, 20));
        assert_ne!(k, thumb_key(p, 10, 21));
        assert_ne!(k, thumb_key(Path::new("/c/b.png"), 10, 20));
    }

    #[test]
    fn stale_thumbnails_are_pruned() {
        let d = temp_dir("thumbs");
        let live = thumb_key(Path::new("/a.png"), 1, 2);
        let dead = thumb_key(Path::new("/b.png"), 1, 2);
        for name in [format!("{live}.png"), format!("{dead}.png"), "notes.png".into(), format!("{dead}.txt")] {
            std::fs::write(d.join(name), b"x").unwrap();
        }
        let keep: HashSet<String> = [live.clone()].into_iter().collect();
        assert_eq!(prune_thumbnails(&d, &keep), 1);
        assert!(d.join(format!("{live}.png")).exists());
        assert!(!d.join(format!("{dead}.png")).exists());
        assert!(d.join("notes.png").exists() && d.join(format!("{dead}.txt")).exists());
        assert_eq!(prune_thumbnails(&d.join("missing"), &keep), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn grid_fits_columns_to_width() {
        let g = Grid::new(860.0);
        assert_eq!(g.columns, 4);
        assert!(g.cell_width >= CELL_MIN_W);
        let right = g.origin(3).0 + g.cell_width;
        assert!((right - (860.0 - g.margin)).abs() < 1e-9, "fills the width");
        assert_eq!(g.origin(4), (g.margin, g.margin + g.cell_height + g.gap));
        assert!((g.thumb_height() - g.cell_width * 0.625).abs() < 1e-9);
        let narrow = Grid::new(100.0);
        assert_eq!(narrow.columns, 1);
        assert_eq!(g.content_height(0, 0.0), 2.0 * g.margin);
        assert_eq!(g.content_height(5, 10.0), 2.0 * g.margin + 2.0 * g.cell_height + g.gap + 10.0);
    }

    #[test]
    fn filters_and_retention_are_localised() {
        use crate::i18n::Language;
        for f in Filter::ALL {
            assert!(!f.msg().text(Language::ZhHans).is_empty());
        }
        for r in Retention::ALL {
            assert_ne!(r.msg().text(Language::En), r.msg().text(Language::ZhHans));
        }
    }
}
