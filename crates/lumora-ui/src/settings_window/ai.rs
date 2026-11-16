//! The Settings window's AI tab: provider preset, endpoint, model, API key
//! (Keychain, via the app), sampling options and "Test connection".
//!
//! The UI never touches the Keychain or the network itself: it emits
//! [`UiEvent::AiKeyRequested`], [`UiEvent::AiKeySave`] and
//! [`UiEvent::AiTestConnection`], and the app answers through
//! `SettingsWindow::set_ai_key`, `set_ai_key_error` and `set_ai_test_result`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lumora_core::ai::{
    connection_message, format_temperature, key_note, model_likely_has_vision, parse_timeout, snap_temperature,
    AiError, AiProvider, OutputLanguage, Secret, TEMPERATURE_RANGE,
};
use lumora_core::geometry::Rect;
use lumora_core::i18n::{tr, Msg};
use lumora_core::settings::Settings;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, MainThreadOnly};
use objc2_app_kit::{
    NSButton, NSColor, NSControlSize, NSFont, NSPopUpButton, NSProgressIndicator, NSProgressIndicatorStyle,
    NSSecureTextField, NSSlider, NSTextField,
};
use objc2_core_foundation::CGPoint;
use objc2_foundation::NSString;

use super::{checkbox, popup, wrapped_height, Form, State, CONTROL_X, NOTE_W, ROW_H};
use crate::events::UiEvent;
use crate::tokens::typography;
use crate::util::{cg_rect, weight, ActionTarget};

/// Index of the AI tab in the tab view.
pub(super) const AI_TAB_INDEX: isize = 4;

/// Live controls of the AI tab, kept so app replies can update them.
pub(super) struct AiControls {
    provider: Retained<NSPopUpButton>,
    base_url: Retained<NSTextField>,
    model: Retained<NSTextField>,
    key: Retained<NSSecureTextField>,
    key_note: Retained<NSTextField>,
    temperature: Retained<NSSlider>,
    temperature_readout: Retained<NSTextField>,
    vision_hint: Retained<NSTextField>,
    timeout: Retained<NSTextField>,
    test: Retained<NSButton>,
    spinner: Retained<NSProgressIndicator>,
    status: Retained<NSTextField>,
    /// The provider whose key the field shows.
    key_provider: Cell<AiProvider>,
    /// Whether the field holds the Keychain value (or a newer edit).
    key_loaded: Cell<bool>,
    /// Last value read from / written to the Keychain.
    saved_key: RefCell<Secret>,
    /// Id of the newest connection test; older replies are ignored.
    test_id: Cell<u64>,
}

impl AiControls {
    /// Ask the app for the current provider's key, once per provider.
    pub(super) fn request_key(&self, state: &State) {
        if !self.key_loaded.get() {
            (state.sink)(UiEvent::AiKeyRequested(self.key_provider.get()));
        }
    }

    /// Write the field to the Keychain if it changed. An empty field
    /// deletes the key, but only once the stored one was loaded (so an
    /// unloaded, empty field never wipes a key).
    pub(super) fn commit_key(&self, state: &State) {
        let value = Secret::new(&self.key.stringValue().to_string());
        if !self.key_loaded.get() && value.is_empty() {
            return;
        }
        if *self.saved_key.borrow() == value {
            return;
        }
        *self.saved_key.borrow_mut() = value.clone();
        self.key_loaded.set(true);
        (state.sink)(UiEvent::AiKeySave { provider: self.key_provider.get(), key: value });
    }

    pub(super) fn set_key(&self, provider: AiProvider, result: Result<Option<String>, AiError>) {
        if provider != self.key_provider.get() {
            return;
        }
        match result {
            Ok(key) => {
                let key = Secret::new(key.as_deref().unwrap_or(""));
                // Don't clobber something typed while the read was queued.
                if self.key.stringValue().to_string().trim().is_empty() {
                    self.key.setStringValue(&NSString::from_str(key.expose()));
                }
                *self.saved_key.borrow_mut() = key;
                self.key_loaded.set(true);
                self.show_key_note(provider);
            }
            Err(e) => self.set_key_error(provider, &e),
        }
    }

    pub(super) fn set_key_error(&self, provider: AiProvider, error: &AiError) {
        if provider != self.key_provider.get() {
            return;
        }
        self.key_note.setStringValue(&NSString::from_str(&error.user_message()));
        self.key_note.setTextColor(Some(&NSColor::systemRedColor()));
    }

    fn show_key_note(&self, provider: AiProvider) {
        self.key_note.setStringValue(&NSString::from_str(key_note(provider).unwrap_or("")));
        self.key_note.setTextColor(Some(&NSColor::secondaryLabelColor()));
    }

    pub(super) fn set_test_result(&self, id: u64, result: Result<Option<bool>, AiError>) {
        if id != self.test_id.get() {
            return;
        }
        self.test.setEnabled(true);
        unsafe { self.spinner.stopAnimation(None) };
        let (text, color) = match result {
            Ok(listed) => (
                connection_message(listed).to_string(),
                if listed == Some(false) { NSColor::systemOrangeColor() } else { NSColor::systemGreenColor() },
            ),
            Err(e) => (e.user_message(), NSColor::systemRedColor()),
        };
        self.status.setStringValue(&NSString::from_str(&text));
        self.status.setTextColor(Some(&color));
    }

    /// Bring every field in line with `s` (after validation filled blanks
    /// or a provider switch changed the presets).
    fn refresh(&self, s: &Settings) {
        let ai = &s.ai;
        if let Some(i) = AiProvider::ALL.iter().position(|p| *p == ai.provider) {
            self.provider.selectItemAtIndex(i as isize);
        }
        set_if_changed(&self.base_url, &ai.base_url);
        set_if_changed(&self.model, &ai.model);
        set_if_changed(&self.timeout, &ai.timeout_secs.to_string());
        self.temperature.setDoubleValue(ai.temperature);
        self.temperature_readout.setStringValue(&NSString::from_str(&format_temperature(ai.temperature)));
        self.vision_hint.setHidden(model_likely_has_vision(ai.provider, &ai.model));
    }
}

fn set_if_changed(field: &NSTextField, value: &str) {
    if field.stringValue().to_string() != value {
        field.setStringValue(&NSString::from_str(value));
    }
}

fn caption(form: &Form<'_>, text: &str, frame: Rect) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), form.mtm);
    l.setFont(Some(&NSFont::systemFontOfSize_weight(typography::CAPTION, weight::REGULAR)));
    l.setTextColor(Some(&NSColor::secondaryLabelColor()));
    l.setFrame(cg_rect(frame));
    form.view.addSubview(&l);
    l
}

/// Send the field's action when editing ends (Tab, click away), not only
/// on Return.
fn action_on_end_editing(field: &NSTextField) {
    if let Some(cell) = field.cell() {
        // SAFETY: NSCell responds to setSendsActionOnEndEditing:.
        let _: () = unsafe { msg_send![&*cell, setSendsActionOnEndEditing: true] };
    }
}

fn text_field(form: &mut Form<'_>, label: &str, value: &str, width: f64) -> Retained<NSTextField> {
    let field = NSTextField::textFieldWithString(&NSString::from_str(value), form.mtm);
    field.setFrame(cg_rect(Rect::new(0.0, 0.0, width, 22.0)));
    action_on_end_editing(&field);
    form.add(label, &field, ROW_H);
    field
}

fn set_action(control: &NSTextField, target: &ActionTarget) {
    // SAFETY: the target is kept alive by the window controller.
    unsafe {
        control.setTarget(Some(target));
        control.setAction(Some(ActionTarget::action()));
    }
}

pub(super) fn build_ai(form: &mut Form<'_>, state: &Rc<State>, targets: &mut Vec<Retained<ActionTarget>>) {
    let mtm = form.mtm;
    let ai = state.settings.borrow().ai.clone();

    checkbox(
        form,
        targets,
        state,
        tr(Msg::AiEnableLabel),
        tr(Msg::AiEnable),
        |s| s.ai.enabled,
        |s, v| s.ai.enabled = v,
    );

    // Provider (custom handler: switches presets and the key field).
    let provider = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        cg_rect(Rect::new(0.0, 0.0, 240.0, 26.0)),
        false,
    );
    for p in AiProvider::ALL {
        provider.addItemWithTitle(&NSString::from_str(p.label()));
    }
    form.add(tr(Msg::AiProviderLabel), &provider, ROW_H);

    let base_url = text_field(form, tr(Msg::AiBaseUrl), &ai.base_url, NOTE_W);
    base_url.setPlaceholderString(Some(&NSString::from_str("https://…/v1")));
    let model = text_field(form, tr(Msg::AiModel), &ai.model, 220.0);

    let key =
        NSSecureTextField::initWithFrame(NSSecureTextField::alloc(mtm), cg_rect(Rect::new(0.0, 0.0, 260.0, 22.0)));
    key.setPlaceholderString(Some(&NSString::from_str(tr(Msg::AiKeyPlaceholder))));
    action_on_end_editing(&key);
    form.add(tr(Msg::AiApiKey), &key, ROW_H);
    form.y -= 18.0;
    let key_note = caption(form, "", Rect::new(CONTROL_X, form.y + 4.0, NOTE_W, 16.0));

    // Temperature slider with a readout to its right.
    let st = state.clone();
    let readout_slot: Rc<RefCell<Option<Retained<NSTextField>>>> = Rc::new(RefCell::new(None));
    let slot = readout_slot.clone();
    let temp_target = ActionTarget::new(mtm, move |sender: &AnyObject| {
        let Some(slider) = sender.downcast_ref::<NSSlider>() else { return };
        let v = snap_temperature(slider.doubleValue());
        if let Some(r) = slot.borrow().as_ref() {
            r.setStringValue(&NSString::from_str(&format_temperature(v)));
        }
        // Save only when the snapped value changes, not on every pixel.
        if (st.settings.borrow().ai.temperature - v).abs() > 1e-9 {
            st.update(|s| s.ai.temperature = v);
        }
    });
    // SAFETY: the target is kept alive by the window controller.
    let temperature = unsafe {
        NSSlider::sliderWithValue_minValue_maxValue_target_action(
            ai.temperature,
            TEMPERATURE_RANGE.0,
            TEMPERATURE_RANGE.1,
            Some(&temp_target),
            Some(ActionTarget::action()),
            mtm,
        )
    };
    temperature.setContinuous(true);
    temperature.setFrame(cg_rect(Rect::new(0.0, 0.0, 180.0, 22.0)));
    form.add(tr(Msg::AiTemperature), &temperature, ROW_H);
    let temperature_readout =
        NSTextField::labelWithString(&NSString::from_str(&format_temperature(ai.temperature)), mtm);
    temperature_readout.setFont(Some(&crate::util::mono_digits(typography::BODY, weight::REGULAR)));
    temperature_readout.setFrame(cg_rect(Rect::new(CONTROL_X + 190.0, form.y + 5.0, 40.0, 20.0)));
    form.view.addSubview(&temperature_readout);
    *readout_slot.borrow_mut() = Some(temperature_readout.clone());
    targets.push(temp_target);

    let languages: Vec<&str> = OutputLanguage::ALL.iter().map(|l| l.label()).collect();
    popup(
        form,
        targets,
        state,
        tr(Msg::AiOutputLanguage),
        &languages,
        |s| OutputLanguage::ALL.iter().position(|l| *l == s.ai.output_language).unwrap_or(0),
        |s, i| s.ai.output_language = OutputLanguage::ALL[i.min(OutputLanguage::ALL.len() - 1)],
    );

    checkbox(
        form,
        targets,
        state,
        tr(Msg::AiImageLabel),
        tr(Msg::AiSendImage),
        |s| s.ai.send_image,
        |s, v| s.ai.send_image = v,
    );
    form.y -= 18.0;
    let vision_hint =
        caption(form, tr(Msg::AiVisionHint), Rect::new(CONTROL_X + 20.0, form.y + 4.0, NOTE_W - 20.0, 16.0));
    vision_hint.setTextColor(Some(&NSColor::systemOrangeColor()));

    checkbox(
        form,
        targets,
        state,
        tr(Msg::AiTutorialLabel),
        tr(Msg::AiTutorialSteps),
        |s| s.ai.tutorial_steps,
        |s, v| s.ai.tutorial_steps = v,
    );

    let timeout = text_field(form, tr(Msg::AiTimeout), &ai.timeout_secs.to_string(), 60.0);
    timeout.setAlignment(objc2_app_kit::NSTextAlignment::Right);
    caption(form, tr(Msg::AiSecondsUnit), Rect::new(CONTROL_X + 68.0, form.y + 7.0, 160.0, 16.0));

    // Test connection: button, spinner, then a wrapping status line.
    let test_slot: Rc<RefCell<Option<std::rc::Weak<AiControls>>>> = Rc::new(RefCell::new(None));
    let slot = test_slot.clone();
    let st = state.clone();
    let test_target = ActionTarget::new(mtm, move |_| {
        let Some(c) = slot.borrow().as_ref().and_then(|w| w.upgrade()) else { return };
        c.commit_key(&st);
        let id = c.test_id.get() + 1;
        c.test_id.set(id);
        c.test.setEnabled(false);
        unsafe { c.spinner.startAnimation(None) };
        c.status.setStringValue(&NSString::from_str(tr(Msg::AiTesting)));
        c.status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let settings = Box::new(st.settings.borrow().ai.clone());
        let key = Secret::new(&c.key.stringValue().to_string());
        (st.sink)(UiEvent::AiTestConnection { id, settings, key });
    });
    // SAFETY: the target is kept alive by the window controller.
    let test = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(tr(Msg::AiTestConnection)),
            Some(&test_target),
            Some(ActionTarget::action()),
            mtm,
        )
    };
    test.sizeToFit();
    form.y -= 6.0;
    form.add("", &test, ROW_H);
    targets.push(test_target);
    let spinner =
        NSProgressIndicator::initWithFrame(NSProgressIndicator::alloc(mtm), cg_rect(Rect::new(0.0, 0.0, 16.0, 16.0)));
    spinner.setStyle(NSProgressIndicatorStyle::Spinning);
    spinner.setControlSize(NSControlSize::Small);
    spinner.setDisplayedWhenStopped(false);
    let tf = test.frame();
    spinner.setFrameOrigin(CGPoint::new(tf.origin.x + tf.size.width + 8.0, form.y + (ROW_H - 16.0) / 2.0));
    form.view.addSubview(&spinner);
    form.y -= 32.0;
    let status = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
    status.setFont(Some(&NSFont::systemFontOfSize_weight(typography::CAPTION, weight::MEDIUM)));
    status.setFrame(cg_rect(Rect::new(CONTROL_X, form.y, NOTE_W, 30.0)));
    status.setSelectable(true);
    form.view.addSubview(&status);

    // Privacy note across the form.
    form.label_only(tr(Msg::AiPrivacyLabel));
    let privacy = NSTextField::wrappingLabelWithString(&NSString::from_str(tr(Msg::AiPrivacyNote)), mtm);
    privacy.setFont(Some(&NSFont::systemFontOfSize_weight(typography::CAPTION, weight::REGULAR)));
    privacy.setTextColor(Some(&NSColor::secondaryLabelColor()));
    let privacy_h = wrapped_height(&privacy, NOTE_W).max(54.0);
    privacy.setFrame(cg_rect(Rect::new(CONTROL_X, form.y + 24.0 - privacy_h, NOTE_W, privacy_h)));
    form.view.addSubview(&privacy);
    form.y -= privacy_h - 24.0;

    let controls = Rc::new(AiControls {
        provider: provider.clone(),
        base_url: base_url.clone(),
        model: model.clone(),
        key: key.clone(),
        key_note,
        temperature,
        temperature_readout,
        vision_hint,
        timeout: timeout.clone(),
        test,
        spinner,
        status,
        key_provider: Cell::new(ai.provider),
        key_loaded: Cell::new(false),
        saved_key: RefCell::new(Secret::default()),
        test_id: Cell::new(0),
    });
    *test_slot.borrow_mut() = Some(Rc::downgrade(&controls));
    controls.show_key_note(ai.provider);
    controls.refresh(&state.settings.borrow());

    // Field handlers. Each holds a weak ref to the controls (which own
    // the fields) to avoid a cycle.
    let weak = Rc::downgrade(&controls);
    let st = state.clone();
    let on_provider = ActionTarget::new(mtm, move |sender: &AnyObject| {
        let (Some(c), Some(p)) = (weak.upgrade(), sender.downcast_ref::<NSPopUpButton>()) else { return };
        let i = p.indexOfSelectedItem().max(0) as usize;
        let provider = AiProvider::ALL[i.min(AiProvider::ALL.len() - 1)];
        if provider == c.key_provider.get() {
            return;
        }
        c.commit_key(&st); // a pending edit belongs to the old provider
        st.update(|s| s.ai.set_provider(provider));
        c.key_provider.set(provider);
        c.key_loaded.set(false);
        *c.saved_key.borrow_mut() = Secret::default();
        c.key.setStringValue(&NSString::from_str(""));
        c.status.setStringValue(&NSString::from_str(""));
        c.show_key_note(provider);
        c.refresh(&st.settings.borrow());
        c.request_key(&st);
    });
    // SAFETY: the target is kept alive by the window controller.
    unsafe {
        provider.setTarget(Some(&on_provider));
        provider.setAction(Some(ActionTarget::action()));
    }
    targets.push(on_provider);

    let field_handler = |apply: fn(&mut Settings, &str) -> bool| {
        let weak = Rc::downgrade(&controls);
        let st = state.clone();
        ActionTarget::new(mtm, move |sender: &AnyObject| {
            let Some(tf) = sender.downcast_ref::<NSTextField>() else { return };
            let v = tf.stringValue().to_string();
            let mut next = st.settings.borrow().clone();
            if apply(&mut next, &v) {
                next.validate();
                if next.ai != st.settings.borrow().ai {
                    st.update(|s| s.ai = next.ai);
                }
            }
            if let Some(c) = weak.upgrade() {
                c.refresh(&st.settings.borrow());
            }
        })
    };
    let t = field_handler(|s, v| {
        s.ai.base_url = v.to_string();
        true
    });
    set_action(&base_url, &t);
    targets.push(t);
    let t = field_handler(|s, v| {
        s.ai.model = v.to_string();
        true
    });
    set_action(&model, &t);
    targets.push(t);
    let t = field_handler(|s, v| match parse_timeout(v) {
        Some(secs) => {
            s.ai.timeout_secs = secs;
            true
        }
        None => false, // refresh reverts the field
    });
    set_action(&timeout, &t);
    targets.push(t);

    let weak = Rc::downgrade(&controls);
    let st = state.clone();
    let on_key = ActionTarget::new(mtm, move |_| {
        if let Some(c) = weak.upgrade() {
            c.commit_key(&st);
        }
    });
    set_action(&key, &on_key);
    targets.push(on_key);

    *state.ai.borrow_mut() = Some(controls);
}
