//! Editable `.lumora` documents: written next to saved screenshots (fresh
//! captures and editor saves), and reopened from the status menu's
//! "Open…", Finder (double-click / Open With) and Quick Access "Annotate".

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_core::annotate::Document;
use lumora_core::beautify::BeautifyStyle;
use lumora_core::geometry::Rect;
use lumora_core::i18n::{tr, trf, Msg};
use lumora_core::lumora_doc::{self, DocError, ImageInfo, LumoraDoc};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGImage;

use crate::app::{on_main, App, CaptureRecord, Editable, Notice};

/// Everything a background `.lumora` write needs (all `Send`).
pub struct DocJob {
    path: PathBuf,
    original: CFRetained<CGImage>,
    png: Option<Arc<Vec<u8>>>,
    scale: f64,
    document: Document,
    /// Only an enabled style is stored.
    beautify: Option<BeautifyStyle>,
    created_at: u64,
}

impl DocJob {
    /// Use already-encoded PNG bytes of the original (skips a second encode).
    pub fn with_png(mut self, png: Arc<Vec<u8>>) -> Self {
        self.png = Some(png);
        self
    }

    /// Encode and write atomically. Returns the original's PNG bytes (to
    /// cache on the record).
    fn run(self) -> Result<Arc<Vec<u8>>, String> {
        let png = match self.png {
            Some(p) => p,
            None => Arc::new(lumora_capture::image::png_bytes(&self.original, self.scale).map_err(|e| e.to_string())?),
        };
        let (w, h) = lumora_capture::image::pixel_size(&self.original);
        let info = ImageInfo { width_px: w as u32, height_px: h as u32, scale: self.scale };
        let doc = LumoraDoc::from_editor_state(&self.document, png.to_vec(), info, self.beautify, self.created_at);
        lumora_doc::write_atomic(&self.path, &doc.encode()).map_err(|e| e.to_string())?;
        Ok(png)
    }
}

/// A `.lumora` file decoded into what the editor needs.
pub struct Loaded {
    pub image: CFRetained<CGImage>,
    pub scale: f64,
    pub png: Arc<Vec<u8>>,
    pub document: Document,
    pub beautify: Option<BeautifyStyle>,
    pub created_at: u64,
}

/// Read and decode `path`. The error is user-facing detail text.
pub fn load_document(path: &Path) -> Result<Loaded, String> {
    let doc = lumora_doc::read(path).map_err(|e| e.to_string())?;
    let document = doc.to_document().map_err(|e| e.to_string())?;
    let image = lumora_capture::image::decode_image(&doc.png).ok_or_else(|| tr(Msg::ErrDocumentImage).to_string())?;
    let (w, h) = lumora_capture::image::pixel_size(&image);
    let info = doc.header.image;
    if (w as u64, h as u64) != (u64::from(info.width_px), u64::from(info.height_px)) {
        return Err(DocError::Invalid(format!(
            "embedded image is {w}×{h} px, header says {}×{}",
            info.width_px, info.height_px
        ))
        .to_string());
    }
    Ok(Loaded {
        image,
        scale: info.scale,
        document,
        beautify: doc.header.beautify,
        created_at: doc.header.created_at,
        png: Arc::new(doc.png),
    })
}

impl App {
    /// The `.lumora` write for `rec`, if it should have one: screenshots
    /// saved to disk with the setting on, or documents opened from disk.
    pub fn doc_job(&self, rec: &CaptureRecord) -> Option<DocJob> {
        let ed = rec.editable.as_ref()?;
        let s = self.settings.borrow();
        if rec.temp || rec.video || !(ed.from_file || s.writes_editable_document()) {
            return None;
        }
        let (w, h) = lumora_capture::image::pixel_size(&ed.original);
        let size = lumora_core::geometry::Size::new(w as f64 / ed.scale, h as f64 / ed.scale);
        let style = ed.beautify.clone().unwrap_or_else(|| s.beautify.clone());
        Some(DocJob {
            path: lumora_doc::doc_path_for(&rec.file),
            original: ed.original.clone(),
            png: ed.png.clone(),
            scale: ed.scale,
            document: ed.document.clone().unwrap_or_else(|| Document::new(size)),
            beautify: style.enabled.then_some(style),
            created_at: ed.created_at,
        })
    }

    /// Run `job` in the background; cache the PNG on success, toast on
    /// failure.
    pub fn spawn_doc_write(self: &Rc<Self>, job: DocJob) {
        let path = job.path.clone();
        DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated)).exec_async(
            move || {
                let result = job.run();
                on_main(move |app| match result {
                    Ok(png) => {
                        log::info!("saved document {}", path.display());
                        app.history_note(path.clone());
                        for rec in app.captures.borrow_mut().values_mut() {
                            if lumora_doc::doc_path_for(&rec.file) == path {
                                if let Some(ed) = rec.editable.as_mut().filter(|e| e.png.is_none()) {
                                    ed.png = Some(png.clone());
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("writing {}: {e}", path.display());
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        app.notify(Notice::Error(Msg::ToastSaveFailed), Some(trf(Msg::ErrWrite, &[&name])));
                    }
                });
            },
        );
    }

    /// Status menu "Open…": choose documents and reopen them.
    pub fn open_document_panel(self: &Rc<Self>) {
        let paths = lumora_ui::util::choose_files_with_extension(self.mtm, lumora_doc::EXTENSION);
        self.open_documents(paths);
    }

    /// Reopen each `.lumora` file in an editor (Finder, Open…).
    pub fn open_documents(self: &Rc<Self>, paths: Vec<PathBuf>) {
        for path in paths {
            self.open_document(&path);
        }
    }

    fn open_document(self: &Rc<Self>, path: &Path) {
        log::info!("opening document {}", path.display());
        let export = lumora_doc::export_path_for(path);
        // Already open: bring that editor forward.
        let open =
            self.captures.borrow().iter().find_map(|(id, r)| {
                (r.file == export && r.editable.as_ref().is_some_and(|e| e.from_file)).then_some(*id)
            });
        if let Some(id) = open.filter(|id| self.editors.borrow().contains_key(id)) {
            return self.open_editor(id);
        }
        let loaded = match load_document(path) {
            Ok(l) => l,
            Err(detail) => return self.document_error(path, &detail),
        };
        let scale = loaded.scale;
        let thumb = lumora_ui::editor::flatten_document(&loaded.image, scale, &loaded.document)
            .unwrap_or_else(|| loaded.image.clone());
        let (w, h) = lumora_capture::image::pixel_size(&thumb);
        let settings_style = self.settings.borrow().beautify.clone();
        let beautify = loaded.beautify.unwrap_or(BeautifyStyle { enabled: false, ..settings_style });
        let id = self.next_id();
        self.captures.borrow_mut().insert(
            id,
            CaptureRecord {
                image: Some(thumb),
                scale,
                frame: Rect::new(0.0, 0.0, w as f64 / scale, h as f64 / scale),
                file: export,
                temp: false,
                video: false,
                editable: Some(Editable {
                    original: loaded.image,
                    scale,
                    png: Some(loaded.png),
                    document: Some(loaded.document),
                    beautify: Some(beautify),
                    created_at: loaded.created_at,
                    from_file: true,
                    ocr: Default::default(),
                }),
            },
        );
        self.open_editor(id);
    }

    /// A `.lumora` file couldn't be read: say so in an alert.
    pub fn document_error(&self, path: &Path, detail: &str) {
        log::error!("opening {}: {detail}", path.display());
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        lumora_ui::util::warning_alert(
            self.mtm,
            &trf(Msg::ErrOpenDocument, &[&name]),
            &trf(Msg::ErrDocumentDetail, &[&detail]),
        );
    }

    /// Quick Access "Annotate" on a capture with nothing drawn in memory:
    /// adopt the `.lumora` next to its file when there is one (it may hold
    /// annotations from an earlier session).
    pub fn hydrate_from_disk(&self, id: u64) {
        let mut caps = self.captures.borrow_mut();
        let Some(rec) = caps.get_mut(&id) else { return };
        if rec.temp || rec.video || rec.editable.as_ref().is_some_and(|e| e.document.is_some()) {
            return;
        }
        let path = lumora_doc::doc_path_for(&rec.file);
        if !path.exists() {
            return;
        }
        match load_document(&path) {
            // An untouched document adds nothing over memory.
            Ok(l) if l.document.items().is_empty() && l.document.crop().is_none() => {}
            Ok(l) => {
                let from_file = rec.editable.as_ref().is_some_and(|e| e.from_file);
                let settings_style = self.settings.borrow().beautify.clone();
                rec.editable = Some(Editable {
                    original: l.image,
                    scale: l.scale,
                    png: Some(l.png),
                    document: Some(l.document),
                    beautify: Some(l.beautify.unwrap_or(BeautifyStyle { enabled: false, ..settings_style })),
                    created_at: l.created_at,
                    from_file,
                    ocr: Default::default(),
                });
            }
            // Fall back to what's in memory; the file may be mid-write.
            Err(e) => log::warn!("ignoring {}: {e}", path.display()),
        }
    }
}
