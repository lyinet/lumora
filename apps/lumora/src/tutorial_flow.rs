//! Recording → step-by-step tutorial.
//!
//! Entry points: the "Make Tutorial" button on a recording's Quick Access
//! card and the menu-bar item "Make Tutorial from Recording…". The click
//! log (`<stem>.events.json`) becomes steps ([`lumora_core::tutorial`]);
//! on a background queue each step gets a keyframe, a ring, a crop and an
//! OCR hint ([`lumora_capture::tutorial`]), then titles from AI (when
//! enabled; text only) or local templates. The result is saved as
//! `<stem>.steps.json` and opened in a [`TutorialWindow`], which exports
//! Markdown / HTML. A menu pick of a recording that already has a
//! `.steps.json` reopens it (with its edits) instead of rebuilding.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_ai::{KeyStore, Keychain};
use lumora_core::click_log::{self, EventLog};
use lumora_core::i18n::{language, tr, trf, Msg};
use lumora_core::tutorial::{self, ai as tutorial_ai, export, BuildOptions, Tutorial};
use lumora_ui::events::TutorialAction;
use lumora_ui::toast::ToastKind;
use lumora_ui::tutorial_window::TutorialWindow;
use lumora_ui::util;

use crate::app::{on_main, App};

/// An open tutorial window and the movie it belongs to.
pub struct OpenTutorial {
    pub window: TutorialWindow,
    pub movie: PathBuf,
}

/// How the background run ended.
enum Outcome {
    Ready { tutorial: Box<Tutorial>, ai_failed: bool, dropped: usize },
    Failed(String),
}

fn base_dir(movie: &Path) -> PathBuf {
    movie.parent().map(Path::to_path_buf).unwrap_or_default()
}

fn stem(movie: &Path) -> String {
    movie.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Tutorial".into())
}

fn save_steps(movie: &Path, t: &Tutorial) {
    let path = tutorial::sidecar_path(movie);
    if let Err(e) = std::fs::write(&path, t.to_json()) {
        log::warn!("could not write {}: {e}", path.display());
    }
}

impl App {
    /// Menu: pick a recording, then make (or reopen) its tutorial.
    pub fn choose_recording_for_tutorial(self: &Rc<Self>) {
        let Some(movie) = util::choose_file_with_extensions(self.mtm, &["mp4", "mov"]) else { return };
        let steps = tutorial::sidecar_path(&movie);
        if steps.exists() {
            match std::fs::read_to_string(&steps)
                .map_err(|e| e.to_string())
                .and_then(|s| Tutorial::from_json(&s).map_err(|e| e.to_string()))
            {
                Ok(t) => return self.open_tutorial_window(movie, t),
                Err(e) => log::warn!("ignoring unreadable {}: {e}", steps.display()),
            }
        }
        self.make_tutorial(movie);
    }

    /// Whether `movie` has a click log to build a tutorial from.
    pub fn can_make_tutorial(movie: &Path) -> bool {
        click_log::sidecar_path(movie).exists()
    }

    /// Build a tutorial for `movie` and open it.
    pub fn make_tutorial(self: &Rc<Self>, movie: PathBuf) {
        if let Some(open) = self.tutorials.borrow().values().find(|o| o.movie == movie) {
            open.window.focus();
            return;
        }
        if !self.tutorial_busy.borrow_mut().insert(movie.clone()) {
            return; // already running for this movie
        }
        let log_path = click_log::sidecar_path(&movie);
        let log = match std::fs::read_to_string(&log_path).ok().and_then(|s| EventLog::from_json(&s).ok()) {
            Some(l) => l,
            None => {
                self.tutorial_busy.borrow_mut().remove(&movie);
                let name = log_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                return self.notify_text(
                    ToastKind::Warning,
                    tr(Msg::ToastTutorialNoLog),
                    Some(&trf(Msg::ToastTutorialNoLogDetail, &[&name])),
                );
            }
        };
        let ui = language();
        let (tutorial, dropped) = tutorial::tutorial_from_log(&log, &BuildOptions::default(), ui);
        if tutorial.steps.is_empty() {
            self.tutorial_busy.borrow_mut().remove(&movie);
            return self.notify_text(
                ToastKind::Warning,
                tr(Msg::ToastTutorialNoClicks),
                Some(tr(Msg::ToastTutorialNoClicksDetail)),
            );
        }

        // AI: settings and key are read here (Keychain on the main thread).
        let mut cfg = self.settings.borrow().ai.clone();
        cfg.validate();
        let ai = if cfg.writes_tutorials() {
            match Keychain.get(cfg.provider) {
                Ok(key) if cfg.check_ready(key.as_deref()).is_ok() => Some((cfg, key)),
                Ok(_) => None,
                Err(e) => {
                    log::warn!("reading the {} API key failed: {e:?}", cfg.provider.id());
                    None
                }
            }
        } else {
            None
        };
        let ai_wanted = self.settings.borrow().ai.writes_tutorials();
        log::info!("making tutorial: {} steps (AI: {})", tutorial.steps.len(), ai.is_some());
        self.notify_text(
            ToastKind::Success,
            tr(Msg::ToastTutorialWorking),
            Some(&trf(Msg::TutorialSummary, &[&tutorial.steps.len()])),
        );

        let movie_bg = movie.clone();
        DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated)).exec_async(
            move || {
                let mut t = tutorial;
                let outcome = match lumora_capture::tutorial::render_steps(&movie_bg, &mut t, |_, _| {}) {
                    Err(e) => Outcome::Failed(e.to_string()),
                    Ok(()) => {
                        // OCR hints are in now: better local titles first.
                        t.apply_local_titles(ui);
                        let mut ai_failed = ai_wanted && ai.is_none();
                        if let Some((cfg, key)) = ai {
                            on_main(|app| app.notify_text(ToastKind::Success, tr(Msg::ToastTutorialWriting), None));
                            let result = tutorial_ai::build_request(&cfg, &t, ui).and_then(|req| {
                                lumora_ai::complete(&cfg, key.as_deref(), &req, &AtomicBool::new(false), |_| {})
                            });
                            match result.and_then(|raw| tutorial_ai::parse_steps(&raw)) {
                                Ok(texts) => {
                                    if tutorial_ai::apply(&mut t, &texts) == 0 {
                                        ai_failed = true;
                                    }
                                }
                                Err(e) => {
                                    log::warn!("tutorial AI write-up failed: {e:?}");
                                    ai_failed = true;
                                }
                            }
                        }
                        save_steps(&movie_bg, &t);
                        Outcome::Ready { tutorial: Box::new(t), ai_failed, dropped }
                    }
                };
                on_main(move |app| app.tutorial_finished(movie_bg, outcome));
            },
        );
    }

    fn tutorial_finished(self: &Rc<Self>, movie: PathBuf, outcome: Outcome) {
        self.tutorial_busy.borrow_mut().remove(&movie);
        match outcome {
            Outcome::Failed(reason) => {
                log::warn!("tutorial failed: {reason}");
                self.notify_text(ToastKind::Error, tr(Msg::ToastTutorialFailed), Some(&reason));
            }
            Outcome::Ready { tutorial, ai_failed, dropped } => {
                if ai_failed {
                    self.notify_text(ToastKind::Warning, tr(Msg::ToastTutorialAiFallback), None);
                } else if dropped > 0 {
                    let kept = tutorial.steps.len();
                    self.notify_text(ToastKind::Warning, &trf(Msg::ToastTutorialCapped, &[&kept]), None);
                } else {
                    self.notify_text(ToastKind::Success, tr(Msg::ToastTutorialReady), None);
                }
                self.open_tutorial_window(movie, *tutorial);
            }
        }
    }

    fn open_tutorial_window(self: &Rc<Self>, movie: PathBuf, t: Tutorial) {
        if let Some(open) = self.tutorials.borrow().values().find(|o| o.movie == movie) {
            open.window.focus();
            return;
        }
        let id = self.next_id();
        let window = TutorialWindow::show(self.mtm, id, t, base_dir(&movie), self.sink.clone());
        self.tutorials.borrow_mut().insert(id, OpenTutorial { window, movie });
    }

    pub fn on_tutorial(self: &Rc<Self>, id: u64, action: TutorialAction) {
        let Some(movie) = self.tutorials.borrow().get(&id).map(|o| o.movie.clone()) else { return };
        let base = base_dir(&movie);
        let ui = language();
        match action {
            TutorialAction::ExportMarkdown(t) => {
                save_steps(&movie, &t);
                let name = format!("{}.md", stem(&movie));
                let Some(path) = util::choose_save_path(self.mtm, &base, &name, "md") else { return };
                let md_dir = path.parent().map(Path::to_path_buf);
                let md = export::to_markdown(&t, ui, |s| {
                    s.image.as_deref().map(|rel| export::image_path_for(md_dir.as_deref(), &base, rel))
                });
                self.finish_export(&path, md.as_bytes());
            }
            TutorialAction::ExportHtml(t) => {
                save_steps(&movie, &t);
                let name = format!("{}.html", stem(&movie));
                let Some(path) = util::choose_save_path(self.mtm, &base, &name, "html") else { return };
                let html =
                    export::to_html(&t, ui, |s| s.image.as_deref().and_then(|rel| std::fs::read(base.join(rel)).ok()));
                self.finish_export(&path, html.as_bytes());
            }
            TutorialAction::CopyMarkdown(t) => {
                save_steps(&movie, &t);
                let md = export::to_markdown(&t, ui, |s| {
                    s.image.as_deref().map(|rel| export::image_path_for(None, &base, rel))
                });
                lumora_capture::clipboard::copy_text(&md);
                self.notify_text(ToastKind::Success, tr(Msg::ToastTutorialCopied), None);
            }
            TutorialAction::Closed(t) => {
                save_steps(&movie, &t);
                // Drop the window after this event (its delegate is mid-close).
                let removed = self.tutorials.borrow_mut().remove(&id);
                drop(removed);
            }
        }
    }

    fn finish_export(self: &Rc<Self>, path: &Path, bytes: &[u8]) {
        match std::fs::write(path, bytes) {
            Ok(()) => {
                log::info!("exported tutorial to {}", path.display());
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                self.notify_text(ToastKind::Success, tr(Msg::ToastTutorialExported), name.as_deref());
                crate::system::reveal_in_finder(path);
            }
            Err(e) => {
                let name = path.display().to_string();
                self.notify_text(ToastKind::Error, tr(Msg::ToastSaveFailed), Some(&format!("{name}: {e}")));
            }
        }
    }
}
