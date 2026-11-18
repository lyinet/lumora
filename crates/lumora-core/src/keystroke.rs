//! Keystroke HUD logic (pure): virtual key code → label, modifier glyphs in
//! Apple's order (⌃⌥⇧⌘), the "what may be shown" rule and repeat merging.
//!
//! Privacy rule: plain typing is never shown. A key press is displayed only
//! when ⌃, ⌥ or ⌘ is held (a shortcut) or when the key itself is a special
//! key (↩ ⇥ ⌫ ⌦ ⎋ arrows, F-keys, Home/End/Page keys). Letters, digits,
//! punctuation and the space bar on their own — with or without ⇧ — are
//! typing and are dropped, so text and passwords never reach the screen.
//! Nothing here stores keystrokes beyond the last label shown.

use serde::{Deserialize, Serialize};

/// How long a label stays on screen (and the window in which an identical
/// press is merged into `×N`), in seconds.
pub const HUD_DURATION: f64 = 1.2;

/// Where the keystroke HUD sits inside the recorded area.
///
/// Unknown values in `settings.toml` (a newer version's position) read as
/// the default instead of failing the whole file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HudPosition {
    #[default]
    BottomCenter,
    BottomLeft,
    BottomRight,
    TopCenter,
}

impl<'de> Deserialize<'de> for HudPosition {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "bottom-left" => HudPosition::BottomLeft,
            "bottom-right" => HudPosition::BottomRight,
            "top-center" => HudPosition::TopCenter,
            _ => HudPosition::BottomCenter,
        })
    }
}

impl HudPosition {
    pub const ALL: [HudPosition; 4] =
        [HudPosition::BottomCenter, HudPosition::BottomLeft, HudPosition::BottomRight, HudPosition::TopCenter];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    pub fn from_index(i: usize) -> HudPosition {
        Self::ALL.get(i).copied().unwrap_or_default()
    }
}

/// Modifier keys held during a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyMods {
    pub control: bool,
    pub option: bool,
    pub shift: bool,
    pub command: bool,
}

impl KeyMods {
    /// From `CGEventFlags` (same bit layout as `NSEventModifierFlags`):
    /// shift 1<<17, control 1<<18, option 1<<19, command 1<<20. The fn /
    /// numeric-pad bits are ignored (arrow keys set them on every press).
    pub fn from_cg_flags(flags: u64) -> KeyMods {
        KeyMods {
            shift: flags & (1 << 17) != 0,
            control: flags & (1 << 18) != 0,
            option: flags & (1 << 19) != 0,
            command: flags & (1 << 20) != 0,
        }
    }

    /// ⌃, ⌥ or ⌘ held — i.e. this is a shortcut, not typing.
    pub fn is_shortcut(self) -> bool {
        self.control || self.option || self.command
    }

    pub fn is_empty(self) -> bool {
        !(self.control || self.option || self.shift || self.command)
    }

    /// Glyphs in Apple's order: ⌃⌥⇧⌘.
    pub fn glyphs(self) -> String {
        let mut s = String::new();
        for (on, g) in [(self.control, '⌃'), (self.option, '⌥'), (self.shift, '⇧'), (self.command, '⌘')] {
            if on {
                s.push(g);
            }
        }
        s
    }
}

/// A key's display label and whether it is a special (non-typing) key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyLabel {
    pub text: &'static str,
    pub special: bool,
}

const fn typing(text: &'static str) -> Option<KeyLabel> {
    Some(KeyLabel { text, special: false })
}

const fn special(text: &'static str) -> Option<KeyLabel> {
    Some(KeyLabel { text, special: true })
}

/// Label for a macOS virtual key code (`kVK_*`), US ANSI layout.
pub fn key_label(keycode: u16) -> Option<KeyLabel> {
    match keycode {
        0 => typing("A"),
        1 => typing("S"),
        2 => typing("D"),
        3 => typing("F"),
        4 => typing("H"),
        5 => typing("G"),
        6 => typing("Z"),
        7 => typing("X"),
        8 => typing("C"),
        9 => typing("V"),
        11 => typing("B"),
        12 => typing("Q"),
        13 => typing("W"),
        14 => typing("E"),
        15 => typing("R"),
        16 => typing("Y"),
        17 => typing("T"),
        18 => typing("1"),
        19 => typing("2"),
        20 => typing("3"),
        21 => typing("4"),
        22 => typing("6"),
        23 => typing("5"),
        24 => typing("="),
        25 => typing("9"),
        26 => typing("7"),
        27 => typing("-"),
        28 => typing("8"),
        29 => typing("0"),
        30 => typing("]"),
        31 => typing("O"),
        32 => typing("U"),
        33 => typing("["),
        34 => typing("I"),
        35 => typing("P"),
        36 => special("↩"),
        37 => typing("L"),
        38 => typing("J"),
        39 => typing("'"),
        40 => typing("K"),
        41 => typing(";"),
        42 => typing("\\"),
        43 => typing(","),
        44 => typing("/"),
        45 => typing("N"),
        46 => typing("M"),
        47 => typing("."),
        48 => special("⇥"),
        // The space bar is typing on its own; shown only in shortcuts (⌘␣).
        49 => typing("␣"),
        50 => typing("`"),
        51 => special("⌫"),
        53 => special("⎋"),
        65 => typing("."),
        67 => typing("*"),
        69 => typing("+"),
        75 => typing("/"),
        76 => special("⌤"),
        78 => typing("-"),
        81 => typing("="),
        82 => typing("0"),
        83 => typing("1"),
        84 => typing("2"),
        85 => typing("3"),
        86 => typing("4"),
        87 => typing("5"),
        88 => typing("6"),
        89 => typing("7"),
        91 => typing("8"),
        92 => typing("9"),
        96 => special("F5"),
        97 => special("F6"),
        98 => special("F7"),
        99 => special("F3"),
        100 => special("F8"),
        101 => special("F9"),
        103 => special("F11"),
        109 => special("F10"),
        111 => special("F12"),
        115 => special("↖"),
        116 => special("⇞"),
        117 => special("⌦"),
        118 => special("F4"),
        119 => special("↘"),
        120 => special("F2"),
        121 => special("⇟"),
        122 => special("F1"),
        123 => special("←"),
        124 => special("→"),
        125 => special("↓"),
        126 => special("↑"),
        _ => None,
    }
}

/// The label to show for one key press, or `None` when it must not be
/// shown (plain typing, unknown key).
pub fn combo_label(keycode: u16, mods: KeyMods) -> Option<String> {
    let key = key_label(keycode)?;
    if !(key.special || mods.is_shortcut()) {
        return None;
    }
    Some(format!("{}{}", mods.glyphs(), key.text))
}

/// Turns key events into HUD text, merging repeats of the same combo.
#[derive(Debug, Default)]
pub struct KeystrokeFeed {
    /// Last combo shown, its repeat count and when it was last pressed.
    last: Option<(String, u32, f64)>,
}

impl KeystrokeFeed {
    pub fn new() -> Self {
        Self::default()
    }

    /// A key-down at `now` (seconds, any monotonic clock). Returns the text
    /// to display, e.g. `"⌘Z"` or `"⌘Z ×3"`, or `None` to show nothing.
    pub fn key_down(&mut self, keycode: u16, mods: KeyMods, now: f64) -> Option<String> {
        let label = combo_label(keycode, mods)?;
        let count = match &self.last {
            Some((prev, n, at)) if *prev == label && now - at <= HUD_DURATION => n + 1,
            _ => 1,
        };
        let text = if count > 1 { format!("{label} ×{count}") } else { label.clone() };
        self.last = Some((label, count, now));
        Some(text)
    }

    /// A modifier change at `now`. While ⌃, ⌥ or ⌘ is held the held
    /// modifiers are previewed (`"⇧⌘"`), so a shortcut builds up on screen;
    /// ⇧ alone is typing and shows nothing. No preview while a combo is
    /// still on screen (it would flicker between `⌘Z ×2` and `⌘`), and
    /// previews never take part in merging.
    pub fn flags_changed(&mut self, mods: KeyMods, now: f64) -> Option<String> {
        let combo_showing = self.last.as_ref().is_some_and(|(_, _, at)| now - at <= HUD_DURATION);
        (mods.is_shortcut() && !combo_showing).then(|| mods.glyphs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD: KeyMods = KeyMods { control: false, option: false, shift: false, command: true };
    const NONE: KeyMods = KeyMods { control: false, option: false, shift: false, command: false };
    const SHIFT: KeyMods = KeyMods { control: false, option: false, shift: true, command: false };

    #[test]
    fn modifiers_in_apple_order() {
        let all = KeyMods { control: true, option: true, shift: true, command: true };
        assert_eq!(all.glyphs(), "⌃⌥⇧⌘");
        let cmd_shift = KeyMods::from_cg_flags((1 << 20) | (1 << 17));
        assert_eq!(combo_label(21, cmd_shift).as_deref(), Some("⇧⌘4"));
        assert_eq!(KeyMods::from_cg_flags((1 << 18) | (1 << 19)).glyphs(), "⌃⌥");
        // fn / numeric-pad bits (arrow keys) are ignored.
        assert!(KeyMods::from_cg_flags((1 << 23) | (1 << 21)).is_empty());
    }

    #[test]
    fn letters_digits_and_specials() {
        assert_eq!(key_label(0).unwrap().text, "A");
        assert_eq!(key_label(29).unwrap().text, "0");
        assert_eq!(key_label(36), Some(KeyLabel { text: "↩", special: true }));
        for (code, text) in [(51, "⌫"), (117, "⌦"), (48, "⇥"), (53, "⎋"), (123, "←"), (126, "↑")] {
            assert_eq!(key_label(code), Some(KeyLabel { text, special: true }), "{code}");
        }
        let fkeys = [122u16, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111];
        for (i, code) in fkeys.iter().enumerate() {
            assert_eq!(key_label(*code).unwrap().text, format!("F{}", i + 1));
        }
        assert_eq!(key_label(49).unwrap().text, "␣");
        assert_eq!(key_label(10), None);
        assert_eq!(key_label(200), None);
    }

    #[test]
    fn plain_typing_is_never_shown() {
        for code in [0u16, 1, 18, 29, 47, 49] {
            assert_eq!(combo_label(code, NONE), None, "{code}");
            assert_eq!(combo_label(code, SHIFT), None, "shift {code}");
        }
        assert_eq!(combo_label(6, CMD).as_deref(), Some("⌘Z"));
        assert_eq!(combo_label(49, CMD).as_deref(), Some("⌘␣"));
        assert_eq!(combo_label(36, NONE).as_deref(), Some("↩"));
        assert_eq!(combo_label(48, SHIFT).as_deref(), Some("⇧⇥"));
        assert_eq!(combo_label(10, CMD), None);
    }

    #[test]
    fn repeats_merge_within_the_window() {
        let mut f = KeystrokeFeed::new();
        assert_eq!(f.key_down(6, CMD, 0.0).as_deref(), Some("⌘Z"));
        assert_eq!(f.key_down(6, CMD, 0.5).as_deref(), Some("⌘Z ×2"));
        assert_eq!(f.key_down(6, CMD, 1.6).as_deref(), Some("⌘Z ×3"));
        // Gap longer than the HUD duration starts over.
        assert_eq!(f.key_down(6, CMD, 3.0).as_deref(), Some("⌘Z"));
        // A different combo resets the count.
        assert_eq!(f.key_down(7, CMD, 3.1).as_deref(), Some("⌘X"));
        assert_eq!(f.key_down(6, CMD, 3.2).as_deref(), Some("⌘Z"));
        // Hidden typing is ignored and does not break the run.
        assert_eq!(f.key_down(0, NONE, 3.3), None);
        assert_eq!(f.key_down(6, CMD, 3.4).as_deref(), Some("⌘Z ×2"));
    }

    #[test]
    fn modifier_preview_only_for_shortcut_modifiers() {
        let mut f = KeystrokeFeed::new();
        assert_eq!(f.flags_changed(SHIFT, 0.0), None);
        assert_eq!(f.flags_changed(NONE, 0.0), None);
        let m = KeyMods { shift: true, command: true, ..NONE };
        assert_eq!(f.flags_changed(m, 0.0).as_deref(), Some("⇧⌘"));
        // While a combo is on screen, modifier changes don't replace it.
        assert_eq!(f.key_down(21, m, 0.1).as_deref(), Some("⇧⌘4"));
        assert_eq!(f.flags_changed(CMD, 0.5), None);
        assert_eq!(f.flags_changed(CMD, 2.0).as_deref(), Some("⌘"));
    }

    #[test]
    fn position_index_round_trip() {
        for p in HudPosition::ALL {
            assert_eq!(HudPosition::from_index(p.index()), p);
        }
        assert_eq!(HudPosition::from_index(99), HudPosition::BottomCenter);
    }
}
