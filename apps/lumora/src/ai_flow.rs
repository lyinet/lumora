//! AI plumbing owned by the app: the Keychain-backed API key and the
//! Settings window's "Test connection". The Settings window only emits
//! events; Keychain and network access happen here (the network off the
//! main thread). Keys are never logged.

use std::rc::Rc;

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_ai::{AiProvider, AiSettings, KeyStore, Keychain, Secret};

use crate::app::{on_main, App};

impl App {
    /// Settings asked for the stored key of `provider`.
    pub fn on_ai_key_requested(self: &Rc<Self>, provider: AiProvider) {
        // Off the main thread: an ad-hoc-signed build may show a Keychain
        // prompt, which would block the run loop (and the Settings window).
        DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated)).exec_async(
            move || {
                let result = Keychain.get(provider);
                if let Err(e) = &result {
                    log::warn!("reading the {} API key from the Keychain failed: {e:?}", provider.id());
                }
                on_main(move |app| {
                    if let Some(w) = app.settings_window.borrow().as_ref() {
                        w.set_ai_key(provider, result);
                    }
                });
            },
        );
    }

    /// Settings wants `key` stored for `provider` (empty = delete).
    pub fn on_ai_key_save(self: &Rc<Self>, provider: AiProvider, key: Secret) {
        let result = if key.is_empty() { Keychain.delete(provider) } else { Keychain.set(provider, key.expose()) };
        match result {
            Ok(()) => log::info!("{} API key {}", provider.id(), if key.is_empty() { "removed" } else { "saved" }),
            Err(e) => {
                log::warn!("saving the {} API key failed: {e:?}", provider.id());
                if let Some(w) = self.settings_window.borrow().as_ref() {
                    w.set_ai_key_error(provider, &e);
                }
            }
        }
    }

    /// "Test connection": blocking HTTP on a background queue, result back
    /// on the main queue to the Settings window (if it is still open).
    pub fn on_ai_test_connection(self: &Rc<Self>, id: u64, settings: AiSettings, key: Secret) {
        DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated)).exec_async(
            move || {
                let result = lumora_ai::test_connection(&settings, key.as_option()).map(|info| info.model_listed);
                match &result {
                    Ok(listed) => log::info!("AI connection test ok (model listed: {listed:?})"),
                    Err(e) => log::info!("AI connection test failed: {e:?}"),
                }
                on_main(move |app| {
                    if let Some(w) = app.settings_window.borrow().as_ref() {
                        w.set_ai_test_result(id, result);
                    }
                });
            },
        );
    }
}
