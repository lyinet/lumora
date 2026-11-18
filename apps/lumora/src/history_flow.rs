//! Capture history (⌥⇧H): the index, the window and its actions.
//!
//! The [`HistoryIndex`] lives behind a mutex and is only touched on
//! background queues: loading, scanning the save folder, reading
//! dimensions, OCR backfill and saving `history.json` never block the main
//! thread. Main keeps the last snapshot the window shows. Thumbnails are
//! rendered off main (ImageIO thumbnails for PNGs, the embedded PNG for
//! `.lumora`, the first frame for movies), cached on disk under
//! `Application Support/Lumora/Thumbnails/<key>.png` and in memory.
//!
//! Deleting only ever moves files to the Trash, after a confirmation.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_core::history::{self, FileClass, FileMeta, HistoryIndex, HistoryItem, ItemKind, NameMatcher};
use lumora_core::i18n::{tr, trf, Msg};
use lumora_core::lumora_doc;
use lumora_ui::events::{HistoryAction, ThumbRequest};
use lumora_ui::history_window::HistoryWindow;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGImage;

use crate::app::{on_main, App, CaptureRecord, Editable, Notice};
use crate::system;

/// Images OCR'd per backfill batch before the window is updated.
const OCR_BATCH: usize = 12;
/// Thumbnails kept in memory.
const THUMB_MEMORY: usize = 600;

/// The index, shared with background work. `None` until first loaded.
type SharedIndex = Arc<Mutex<Option<HistoryIndex>>>;

#[derive(Default)]
pub struct HistoryState {
    window: Option<HistoryWindow>,
    /// What the window shows (newest first).
    snapshot: Vec<HistoryItem>,
    index: SharedIndex,
    /// Bumped whenever the window closes, so late thumbnails / OCR batches
    /// for a closed window are dropped.
    generation: Arc<AtomicU64>,
    thumbs: HashMap<String, CFRetained<CGImage>>,
    ocr_running: bool,
    refresh_running: bool,
    refresh_again: bool,
}

fn utility() -> dispatch2::DispatchRetained<DispatchQueue> {
    DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::Utility))
}

fn user_initiated() -> dispatch2::DispatchRetained<DispatchQueue> {
    DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated))
}

/// Run `f` on the loaded index (loading it first), then save it when `f`
/// says it changed. Call off the main thread.
fn with_index<R>(shared: &SharedIndex, path: &Path, f: impl FnOnce(&mut HistoryIndex) -> (R, bool)) -> R {
    let mut guard = shared.lock().unwrap_or_else(|p| p.into_inner());
    let index = guard.get_or_insert_with(|| HistoryIndex::load_or_default(path));
    let (r, changed) = f(index);
    if changed {
        if let Err(e) = index.save(path) {
            log::warn!("saving the history index failed: {e}");
        }
    }
    r
}

/// The history item for a file Lumora just wrote, if it is one.
fn item_for(path: &Path) -> Option<HistoryItem> {
    match history::classify(path)? {
        FileClass::Image => {
            let meta = FileMeta::of(path)?;
            let doc = lumora_doc::doc_path_for(path);
            let document = FileMeta::of(&doc).map(|_| doc);
            let mut item = HistoryItem { document, ..HistoryItem::new(path.to_path_buf(), ItemKind::Screenshot, meta) };
            if let Some((w, h)) = history::read_dimensions(path, ItemKind::Screenshot) {
                (item.width_px, item.height_px) = (Some(w), Some(h));
            }
            Some(item)
        }
        FileClass::Document => {
            let png = lumora_doc::export_path_for(path);
            if FileMeta::of(&png).is_some() {
                return item_for(&png);
            }
            let mut item = HistoryItem::new(path.to_path_buf(), ItemKind::Document, FileMeta::of(path)?);
            if let Some((w, h)) = history::read_dimensions(path, ItemKind::Document) {
                (item.width_px, item.height_px) = (Some(w), Some(h));
            }
            Some(item)
        }
        FileClass::Movie => {
            let mut item = HistoryItem::new(path.to_path_buf(), ItemKind::Recording, FileMeta::of(path)?);
            if let Some((w, h)) = lumora_capture::recorder::video_dimensions(path) {
                (item.width_px, item.height_px) = (Some(w), Some(h));
            }
            Some(item)
        }
    }
}

/// Render a thumbnail for `req`, from the disk cache when it is there.
fn make_thumbnail(req: &ThumbRequest, cache_dir: &Path) -> Option<CFRetained<CGImage>> {
    let cached = cache_dir.join(format!("{}.png", req.key));
    if let Some(img) = lumora_capture::image::thumbnail_file(&cached, history::THUMB_MAX_PX) {
        return Some(img);
    }
    let max = history::THUMB_MAX_PX;
    let img = match req.kind {
        ItemKind::Screenshot => lumora_capture::image::thumbnail_file(&req.path, max),
        ItemKind::Document => {
            lumora_doc::read(&req.path).ok().and_then(|d| lumora_capture::image::thumbnail_data(&d.png, max))
        }
        ItemKind::Recording => lumora_capture::recorder::thumbnail(&req.path, f64::from(max)),
    }?;
    match lumora_capture::image::png_bytes(&img, 1.0) {
        Ok(png) => {
            if let Err(e) = lumora_doc::write_atomic(&cached, &png) {
                log::debug!("thumbnail cache write failed: {e}");
            }
        }
        Err(e) => log::debug!("thumbnail encode failed: {e}"),
    }
    Some(img)
}

/// The full image of a screenshot or document, for OCR.
fn full_image(path: &Path, kind: ItemKind) -> Option<CFRetained<CGImage>> {
    match kind {
        ItemKind::Screenshot => lumora_capture::image::read_file(path).map(|(img, _)| img),
        ItemKind::Document => lumora_doc::read(path).ok().and_then(|d| lumora_capture::image::decode_image(&d.png)),
        ItemKind::Recording => None,
    }
}

impl App {
    fn history_index_path(&self) -> PathBuf {
        history::index_path(&self.home)
    }

    /// Whether the history window is open.
    pub fn history_open(&self) -> bool {
        self.history.borrow().window.is_some()
    }

    /// Status menu / ⌥⇧H: open (or bring forward) the history window.
    pub fn show_history(self: &Rc<Self>) {
        if let Some(w) = self.history.borrow().window.as_ref() {
            w.focus();
            return;
        }
        let window = HistoryWindow::show(self.mtm, self.sink.clone());
        self.history.borrow_mut().window = Some(window);
        self.refresh_history();
    }

    /// Rescan the save folder and merge it into the index (background),
    /// then show the result.
    pub fn refresh_history(self: &Rc<Self>) {
        {
            let mut h = self.history.borrow_mut();
            if h.refresh_running {
                h.refresh_again = true;
                return;
            }
            h.refresh_running = true;
        }
        let s = self.settings.borrow();
        let folder = s.save_folder(&self.home);
        let names = NameMatcher::new(&s.general.filename_template);
        drop(s);
        let shared = self.history.borrow().index.clone();
        let path = self.history_index_path();
        let thumbs_dir = history::thumbnail_dir(&self.home);
        utility().exec_async(move || {
            let scanned = history::scan_folder(&folder, &names);
            let movies = with_index(&shared, &path, |idx| {
                idx.refresh(scanned, FileMeta::of);
                for (p, kind) in idx.missing_dimensions() {
                    if let Some((w, h)) = history::read_dimensions(&p, kind) {
                        idx.set_dimensions(&p, w, h);
                    }
                }
                let movies: Vec<PathBuf> = idx
                    .items
                    .iter()
                    .filter(|i| i.kind == ItemKind::Recording && i.dimensions().is_none())
                    .map(|i| i.path.clone())
                    .collect();
                (movies, true)
            });
            // AVFoundation reads happen outside the lock.
            let dims: Vec<(PathBuf, (u32, u32))> = movies
                .into_iter()
                .filter_map(|p| lumora_capture::recorder::video_dimensions(&p).map(|d| (p, d)))
                .collect();
            let snapshot = with_index(&shared, &path, |idx| {
                let mut changed = false;
                for (p, (w, h)) in &dims {
                    changed |= idx.set_dimensions(p, *w, *h);
                }
                (idx.items.clone(), changed)
            });
            let live: HashSet<String> = snapshot.iter().map(HistoryItem::thumb_key).collect();
            let pruned = history::prune_thumbnails(&thumbs_dir, &live);
            if pruned > 0 {
                log::info!("removed {pruned} stale history thumbnail(s)");
            }
            on_main(move |app| {
                let again = {
                    let mut h = app.history.borrow_mut();
                    h.refresh_running = false;
                    std::mem::take(&mut h.refresh_again)
                };
                app.history_loaded(snapshot);
                if again {
                    app.refresh_history();
                }
            });
        });
    }

    /// Show a fresh snapshot in the window (if open) and keep OCR going.
    fn history_loaded(self: &Rc<Self>, items: Vec<HistoryItem>) {
        let retention = self.settings.borrow().history.retention;
        let expired = history::expired(&items, system::unix_now(), retention).len();
        let days = retention.days().unwrap_or(0);
        {
            let h = self.history.borrow();
            if let Some(w) = h.window.as_ref() {
                w.set_items(items.clone(), expired, days);
            }
        }
        self.history.borrow_mut().snapshot = items;
        self.continue_history_ocr();
    }

    /// Lumora wrote `path` (a capture, an editor save, a `.lumora`, a
    /// recording, or a capture moved out of the scratch folder): record it.
    pub fn history_note(self: &Rc<Self>, path: PathBuf) {
        if path.starts_with(system::temp_dir()) {
            return;
        }
        let shared = self.history.borrow().index.clone();
        let index_path = self.history_index_path();
        utility().exec_async(move || {
            let Some(item) = item_for(&path) else { return };
            let snapshot = with_index(&shared, &index_path, |idx| {
                // A document whose PNG reappeared is now listed with it.
                if let Some(doc) = item.document.as_deref() {
                    idx.remove(doc);
                }
                idx.upsert(item);
                (idx.items.clone(), true)
            });
            on_main(move |app| {
                if app.history.borrow().window.is_some() {
                    app.history_loaded(snapshot);
                }
            });
        });
    }

    /// Lumora moved `path` to the Trash itself (Quick Access Delete).
    pub fn history_forget(&self, path: PathBuf) {
        let shared = self.history.borrow().index.clone();
        let index_path = self.history_index_path();
        utility().exec_async(move || {
            with_index(&shared, &index_path, |idx| ((), idx.remove(&path)));
        });
    }

    /// Text was recognised on the capture saved as `path`: keep it for
    /// search.
    pub fn history_note_ocr(&self, path: PathBuf, text: String) {
        if path.starts_with(system::temp_dir()) {
            return;
        }
        let shared = self.history.borrow().index.clone();
        let index_path = self.history_index_path();
        utility().exec_async(move || {
            with_index(&shared, &index_path, |idx| ((), idx.set_ocr_text(&path, &text)));
        });
    }

    /// While the window is open, recognise text on screenshots that have
    /// none yet, a batch at a time (newest first), so search finds them.
    fn continue_history_ocr(self: &Rc<Self>) {
        let (batch, generation, shared) = {
            let h = self.history.borrow();
            if h.ocr_running || h.window.is_none() {
                return;
            }
            let batch: Vec<(PathBuf, ItemKind)> =
                h.snapshot.iter().filter(|i| i.wants_ocr()).take(OCR_BATCH).map(|i| (i.path.clone(), i.kind)).collect();
            (batch, h.generation.clone(), h.index.clone())
        };
        if batch.is_empty() {
            return;
        }
        self.history.borrow_mut().ocr_running = true;
        let gen = generation.load(Ordering::SeqCst);
        let index_path = self.history_index_path();
        utility().exec_async(move || {
            let mut results = Vec::new();
            for (path, kind) in batch {
                if generation.load(Ordering::SeqCst) != gen {
                    break;
                }
                // An unreadable image or a Vision error still counts as
                // "looked": it is retried only when the file changes.
                let text = full_image(&path, kind)
                    .and_then(|img| lumora_capture::ocr::recognize_sync(&img).ok())
                    .unwrap_or_default();
                results.push((path, text));
            }
            let snapshot = with_index(&shared, &index_path, |idx| {
                let mut changed = false;
                for (p, t) in &results {
                    changed |= idx.set_ocr_text(p, t);
                }
                (idx.items.clone(), changed)
            });
            let stopped = generation.load(Ordering::SeqCst) != gen;
            on_main(move |app| {
                app.history.borrow_mut().ocr_running = false;
                if !stopped && app.history.borrow().window.is_some() {
                    app.history_loaded(snapshot);
                }
            });
        });
    }

    pub fn on_history(self: &Rc<Self>, action: HistoryAction) {
        match action {
            HistoryAction::Closed => {
                let mut h = self.history.borrow_mut();
                h.generation.fetch_add(1, Ordering::SeqCst);
                let w = h.window.take();
                drop(h);
                drop(w);
            }
            HistoryAction::NeedThumbnails(reqs) => self.history_thumbnails(reqs),
            HistoryAction::Reopen(path) => self.history_reopen(&path),
            HistoryAction::Copy(path) => self.history_copy(&path),
            HistoryAction::Reveal(path) => {
                if self.history_check_exists(&path) {
                    system::reveal_in_finder(&path);
                }
            }
            HistoryAction::Delete(path) => self.history_delete(&path),
            HistoryAction::Trim(path) => {
                if self.history_check_exists(&path) {
                    self.open_trim(path, false, None);
                }
            }
            HistoryAction::TrashExpired => self.history_trash_expired(),
        }
    }

    fn history_item(&self, path: &Path) -> Option<HistoryItem> {
        self.history.borrow().snapshot.iter().find(|i| i.path == path).cloned()
    }

    /// `false` (with a toast and a rescan) when the file has gone.
    fn history_check_exists(self: &Rc<Self>, path: &Path) -> bool {
        if path.exists() {
            return true;
        }
        self.notify(Notice::Warning(Msg::HistoryMissingFile), None);
        self.refresh_history();
        false
    }

    fn history_thumbnails(self: &Rc<Self>, reqs: Vec<ThumbRequest>) {
        let mut todo = Vec::new();
        {
            let h = self.history.borrow();
            let Some(w) = h.window.as_ref() else { return };
            for r in reqs {
                match h.thumbs.get(&r.key) {
                    Some(img) => w.set_thumbnail(&r.key, img),
                    None => todo.push(r),
                }
            }
        }
        if todo.is_empty() {
            return;
        }
        let generation = self.history.borrow().generation.clone();
        let gen = generation.load(Ordering::SeqCst);
        let cache_dir = history::thumbnail_dir(&self.home);
        user_initiated().exec_async(move || {
            for req in todo {
                if generation.load(Ordering::SeqCst) != gen {
                    return;
                }
                let img = make_thumbnail(&req, &cache_dir);
                on_main(move |app| {
                    let mut h = app.history.borrow_mut();
                    match img {
                        Some(img) => {
                            if h.thumbs.len() >= THUMB_MEMORY {
                                h.thumbs.clear();
                            }
                            if let Some(w) = h.window.as_ref() {
                                w.set_thumbnail(&req.key, &img);
                            }
                            h.thumbs.insert(req.key, img);
                        }
                        None => {
                            if let Some(w) = h.window.as_ref() {
                                w.thumbnail_failed(&req.key);
                            }
                        }
                    }
                });
            }
        });
    }

    fn history_reopen(self: &Rc<Self>, path: &Path) {
        if !self.history_check_exists(path) {
            return;
        }
        let item = self.history_item(path);
        let kind = item.as_ref().map_or(ItemKind::Screenshot, |i| i.kind);
        if kind == ItemKind::Recording {
            lumora_ui::quick_access::open_file(path);
            return;
        }
        // The editable document wins: every annotation stays editable.
        if let Some(doc) = item.as_ref().and_then(|i| i.editable_document()).filter(|d| d.exists()) {
            self.open_documents(vec![doc.to_path_buf()]);
            return;
        }
        self.open_image_in_editor(path.to_path_buf());
    }

    /// Open a plain PNG (no `.lumora`) in the editor; saving writes the PNG
    /// back (and a `.lumora` beside it when that setting is on).
    fn open_image_in_editor(self: &Rc<Self>, path: PathBuf) {
        let open = self.captures.borrow().iter().find_map(|(id, r)| (r.file == path && !r.video).then_some(*id));
        if let Some(id) = open.filter(|id| self.editors.borrow().contains_key(id)) {
            return self.open_editor(id);
        }
        user_initiated().exec_async(move || {
            let loaded = lumora_capture::image::read_file(&path);
            on_main(move |app| {
                let Some((image, scale)) = loaded else {
                    app.document_error(&path, tr(Msg::ErrDocumentImage));
                    return;
                };
                let (w, h) = lumora_capture::image::pixel_size(&image);
                let id = app.next_id();
                app.captures.borrow_mut().insert(
                    id,
                    CaptureRecord {
                        image: Some(image.clone()),
                        scale,
                        frame: lumora_core::geometry::Rect::new(0.0, 0.0, w as f64 / scale, h as f64 / scale),
                        file: path,
                        temp: false,
                        video: false,
                        editable: Some(Editable::new(image, scale)),
                    },
                );
                app.open_editor(id);
            });
        });
    }

    fn history_copy(self: &Rc<Self>, path: &Path) {
        if !self.history_check_exists(path) {
            return;
        }
        let kind = self.history_item(path).map_or(ItemKind::Screenshot, |i| i.kind);
        if kind == ItemKind::Recording {
            lumora_capture::clipboard::copy_file(path);
            self.notify(Notice::Success(Msg::ToastCopied), None);
            return;
        }
        let path = path.to_path_buf();
        user_initiated().exec_async(move || {
            // A screenshot's PNG as it is on disk; an orphaned document's
            // original capture.
            let png = match kind {
                ItemKind::Document => lumora_doc::read(&path).ok().map(|d| (d.png, None)),
                _ => std::fs::read(&path).ok().map(|b| (b, Some(path.clone()))),
            };
            on_main(move |app| match png {
                Some((bytes, file)) => {
                    lumora_capture::clipboard::copy_png(&bytes, file.as_deref());
                    app.notify(Notice::Success(Msg::ToastCopied), None);
                }
                None => app.notify(Notice::Warning(Msg::HistoryMissingFile), None),
            });
        });
    }

    /// Move an item's files to the Trash; `Err` names the first that
    /// couldn't be moved. Never deletes.
    fn trash_item_files(item: &HistoryItem) -> Result<(), String> {
        for f in item.files() {
            if f.exists() && !system::move_to_trash(&f) {
                return Err(f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            }
        }
        Ok(())
    }

    fn history_delete(self: &Rc<Self>, path: &Path) {
        let Some(item) = self.history_item(path) else { return };
        let confirmed = lumora_ui::util::confirm_alert(
            self.mtm,
            &trf(Msg::HistoryDeleteTitle, &[&item.file_name()]),
            tr(Msg::HistoryDeleteDetail),
            tr(Msg::HistoryMoveToTrash),
            tr(Msg::HistoryCancel),
        );
        if !confirmed {
            return;
        }
        self.history_trash(vec![item]);
    }

    fn history_trash_expired(self: &Rc<Self>) {
        let retention = self.settings.borrow().history.retention;
        let Some(days) = retention.days() else { return };
        let items: Vec<HistoryItem> = {
            let h = self.history.borrow();
            history::expired(&h.snapshot, system::unix_now(), retention).into_iter().cloned().collect()
        };
        if items.is_empty() {
            return;
        }
        let n = items.len();
        let confirmed = lumora_ui::util::confirm_alert(
            self.mtm,
            &trf(Msg::HistoryRetentionConfirmTitle, &[&n, &days]),
            tr(Msg::HistoryRetentionConfirmDetail),
            tr(Msg::HistoryMoveToTrash),
            tr(Msg::HistoryCancel),
        );
        if confirmed {
            self.history_trash(items);
        }
    }

    /// Trash each item's files (on main: `NSFileManager` is quick for local
    /// files), forget them, and refresh.
    fn history_trash(self: &Rc<Self>, items: Vec<HistoryItem>) {
        let mut failed = None;
        let mut gone = Vec::new();
        for item in items {
            match Self::trash_item_files(&item) {
                Ok(()) => gone.push(item.path.clone()),
                Err(name) => {
                    log::warn!("could not move {name} to the Trash");
                    failed.get_or_insert(name);
                }
            }
        }
        if let Some(name) = failed {
            self.notify(Notice::Error(Msg::HistoryTrashFailed), Some(trf(Msg::HistoryTrashFailedDetail, &[&name])));
        }
        let shared = self.history.borrow().index.clone();
        let index_path = self.history_index_path();
        utility().exec_async(move || {
            with_index(&shared, &index_path, |idx| {
                let mut changed = false;
                for p in &gone {
                    changed |= idx.remove(p);
                }
                ((), changed)
            });
            on_main(|app| app.refresh_history());
        });
    }
}
