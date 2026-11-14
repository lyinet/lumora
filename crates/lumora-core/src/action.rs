//! The user-facing commands that can be bound to global shortcuts.

use serde::{Deserialize, Serialize};

use crate::i18n::{tr, Msg};

/// A bindable action. The app maps each to a `Command` on its dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    CaptureArea,
    CaptureFullscreen,
    CaptureWindow,
    /// Select an area and copy the text in it (OCR).
    CaptureText,
    RecordScreen,
    OpenFolder,
}

impl Action {
    pub const ALL: [Action; 6] = [
        Action::CaptureArea,
        Action::CaptureFullscreen,
        Action::CaptureWindow,
        Action::CaptureText,
        Action::RecordScreen,
        Action::OpenFolder,
    ];

    /// The localisation key of the title.
    pub fn msg(self) -> Msg {
        match self {
            Action::CaptureArea => Msg::CaptureArea,
            Action::CaptureFullscreen => Msg::CaptureFullscreen,
            Action::CaptureWindow => Msg::CaptureWindow,
            Action::CaptureText => Msg::CaptureText,
            Action::RecordScreen => Msg::RecordScreen,
            Action::OpenFolder => Msg::OpenCapturesFolder,
        }
    }

    /// Localised title used in menus and the Shortcuts tab.
    pub fn title(self) -> &'static str {
        tr(self.msg())
    }

    /// Default shortcut. Option+Shift avoids the system's ⇧⌘3/4/5.
    pub fn default_shortcut(self) -> &'static str {
        match self {
            Action::CaptureArea => "⌥⇧4",
            Action::CaptureFullscreen => "⌥⇧3",
            Action::CaptureWindow => "⌥⇧5",
            Action::CaptureText => "⌥⇧2",
            Action::RecordScreen => "⌥⇧R",
            Action::OpenFolder => "⌥⇧O",
        }
    }

    /// Stable numeric id, used as the Carbon hotkey id.
    pub fn id(self) -> u32 {
        Action::ALL.iter().position(|a| *a == self).unwrap_or(0) as u32 + 1
    }

    pub fn from_id(id: u32) -> Option<Action> {
        Action::ALL.get((id as usize).checked_sub(1)?).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for a in Action::ALL {
            assert_eq!(Action::from_id(a.id()), Some(a));
        }
        assert_eq!(Action::from_id(0), None);
        assert_eq!(Action::from_id(99), None);
    }

    #[test]
    fn titles_are_localised() {
        for a in Action::ALL {
            assert!(!a.msg().text(crate::i18n::Language::ZhHans).is_empty());
        }
        assert_eq!(Action::CaptureText.msg().text(crate::i18n::Language::En), "Capture Text");
    }
}
