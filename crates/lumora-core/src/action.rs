//! The user-facing commands that can be bound to global shortcuts.

use serde::{Deserialize, Serialize};

/// A bindable action. The app maps each to a `Command` on its dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    CaptureArea,
    CaptureFullscreen,
    CaptureWindow,
    RecordScreen,
    OpenFolder,
}

impl Action {
    pub const ALL: [Action; 5] = [
        Action::CaptureArea,
        Action::CaptureFullscreen,
        Action::CaptureWindow,
        Action::RecordScreen,
        Action::OpenFolder,
    ];

    /// Human-readable title used in menus and the Shortcuts tab.
    pub fn title(self) -> &'static str {
        match self {
            Action::CaptureArea => "Capture Area",
            Action::CaptureFullscreen => "Capture Fullscreen",
            Action::CaptureWindow => "Capture Window",
            Action::RecordScreen => "Record Screen",
            Action::OpenFolder => "Open Captures Folder",
        }
    }

    /// Default shortcut. Option+Shift avoids the system's ⇧⌘3/4/5.
    pub fn default_shortcut(self) -> &'static str {
        match self {
            Action::CaptureArea => "⌥⇧4",
            Action::CaptureFullscreen => "⌥⇧3",
            Action::CaptureWindow => "⌥⇧5",
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
}
