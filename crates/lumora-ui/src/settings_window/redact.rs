//! Settings → Capture: the Smart Redaction section (detector categories
//! two per row, faces, redaction style, AI check, auto-redact).

use std::rc::Rc;

use lumora_core::annotate::RedactMode;
use lumora_core::i18n::{tr, Msg};
use lumora_core::redact::Category;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSButton, NSControlStateValueOff, NSControlStateValueOn};
use objc2_core_foundation::CGPoint;
use objc2_foundation::NSString;

use super::{checkbox, popup, Form, State, CONTROL_X, ROW_H};
use crate::util::ActionTarget;

/// Column offset of the second checkbox in a row.
const COLUMN: f64 = 140.0;

fn category_label(c: Category) -> Msg {
    match c {
        Category::Email => Msg::RedactEmail,
        Category::Phone => Msg::RedactPhone,
        Category::IdCard => Msg::RedactIdCard,
        Category::BankCard => Msg::RedactBankCard,
        Category::Secret => Msg::RedactSecret,
        Category::IpAddress => Msg::RedactIp,
        Category::UrlToken => Msg::RedactUrlToken,
        Category::Face | Category::Other => Msg::RedactFaces,
    }
}

fn category_box(
    form: &Form<'_>,
    state: &Rc<State>,
    c: Category,
    targets: &mut Vec<Retained<ActionTarget>>,
) -> Retained<NSButton> {
    let st = state.clone();
    let t = ActionTarget::new(form.mtm, move |sender: &AnyObject| {
        let on = sender.downcast_ref::<NSButton>().is_some_and(|b| b.state() == NSControlStateValueOn);
        st.update(|s| s.redact.set(c, on));
    });
    // SAFETY: target kept alive by the window controller.
    let b = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str(tr(category_label(c))),
            Some(&t),
            Some(ActionTarget::action()),
            form.mtm,
        )
    };
    let on = state.settings.borrow().redact.enabled(c);
    b.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
    b.sizeToFit();
    targets.push(t);
    b
}

pub(super) fn build_redact(form: &mut Form<'_>, state: &Rc<State>, targets: &mut Vec<Retained<ActionTarget>>) {
    let mut cats: Vec<Category> = Category::TEXT.to_vec();
    cats.push(Category::Face);
    for (row, pair) in cats.chunks(2).enumerate() {
        let label = if row == 0 { tr(Msg::RedactSectionLabel) } else { "" };
        let first = category_box(form, state, pair[0], targets);
        form.add(label, &first, ROW_H);
        if let Some(&c) = pair.get(1) {
            let second = category_box(form, state, c, targets);
            let mut f = second.frame();
            f.origin = CGPoint::new(CONTROL_X + COLUMN, first.frame().origin.y);
            second.setFrame(f);
            form.view.addSubview(&second);
        }
    }
    popup(
        form,
        targets,
        state,
        tr(Msg::RedactStyleLabel),
        &[tr(Msg::OptionPixelate), tr(Msg::OptionBlur)],
        |s| usize::from(s.redact.mode == RedactMode::Blur),
        |s, i| s.redact.mode = if i == 1 { RedactMode::Blur } else { RedactMode::Pixelate },
    );
    checkbox(form, targets, state, "", tr(Msg::RedactAiCheck), |s| s.redact.ai_check, |s, v| s.redact.ai_check = v);
    checkbox(form, targets, state, "", tr(Msg::RedactAuto), |s| s.redact.auto_redact, |s, v| s.redact.auto_redact = v);
    form.note(tr(Msg::RedactNote));
}
