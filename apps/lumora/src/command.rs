//! The central command vocabulary. Menu items, global hotkeys and UI events
//! all funnel into `App::dispatch(Command)`, so every entry point behaves
//! identically.

use lumora_core::Action;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum Command {
    CaptureArea,
    CaptureFullscreen,
    CaptureWindow,
    /// Select an area and copy its text (OCR).
    CaptureText,
    /// Start (choose area) or stop a recording.
    ToggleRecording,
    OpenFolder,
    /// The capture history window.
    ShowHistory,
    /// Choose `.lumora` documents to reopen in the editor.
    OpenDocument,
    /// Choose a recording and make a step-by-step tutorial from it.
    MakeTutorial,
    /// Choose a recording and transcribe it (or cancel running transcriptions).
    Transcribe,
    ShowSettings,
    ShowOnboarding,
    Quit,
}

impl From<Action> for Command {
    fn from(a: Action) -> Self {
        match a {
            Action::CaptureArea => Command::CaptureArea,
            Action::CaptureFullscreen => Command::CaptureFullscreen,
            Action::CaptureWindow => Command::CaptureWindow,
            Action::CaptureText => Command::CaptureText,
            Action::RecordScreen => Command::ToggleRecording,
            Action::OpenFolder => Command::OpenFolder,
            Action::History => Command::ShowHistory,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_maps_to_a_distinct_command() {
        let cmds: Vec<Command> = Action::ALL.iter().map(|&a| a.into()).collect();
        for (i, a) in cmds.iter().enumerate() {
            assert!(cmds.iter().skip(i + 1).all(|b| b != a));
        }
    }
}
