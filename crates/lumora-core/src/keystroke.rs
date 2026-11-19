//! Keystroke HUD logic (pure): virtual key code → label, modifier glyphs in
//! Apple's order (⌃⌥⇧⌘), the "what may be shown" rule and repeat merging.
//!
//! Privacy rule: plain typing is never shown. A key press is displayed only
//! when ⌃, ⌥ or ⌘ is held (a shortcut) or when the key itself is a special
//! key (↩ ⇥ ⌫ ⌦ ⎋ arrows, F-keys, Home/End/Page keys). Letters, digits,
//! punctuation and the space bar on their own — with or without ⇧ — are
//! typing and are dropped, so text and passwords never reach the screen.
//! Nothing here stores keystrokes beyond the last label shown.
//!
//! Keyboard layouts: the character on a key comes from a [`KeyTranslator`]
//! (on macOS, `UCKeyTranslate` on the current layout; see
//! [`translation_plan`] for which layout and modifier state are used).
//! Named keys (↩ ⇥ ⌫ ⎋ ␣ arrows, F-keys…) always come from the built-in
//! table, and the US table is the fallback whenever a translation fails or
//! yields nothing printable. Plain typing is dropped *before* any
//! translation, so the translator never sees it.

use std::collections::HashMap;

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

/// Space bar key code (`kVK_Space`).
const KEY_SPACE: u16 = 49;

/// A key whose label never depends on the keyboard layout: the special keys
/// of the built-in table and the space bar.
pub fn is_named_key(keycode: u16) -> bool {
    keycode == KEY_SPACE || key_label(keycode).is_some_and(|k| k.special)
}

/// Which keyboard layout a key is translated with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutKind {
    /// The selected keyboard layout (for an input method such as Pinyin, the
    /// layout it types through).
    Current,
    /// The ASCII-capable layout macOS uses for ⌘ shortcuts when the selected
    /// layout is not Latin (Russian, Greek…); the same as `Current` when it is.
    AsciiCapable,
}

/// Modifier state handed to the layout translation. ⇧ is never passed: the
/// HUD shows ⇧ as a glyph next to the unshifted key (`⇧⌘4`, not `⇧⌘$`),
/// like menus do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TranslateMods {
    /// No modifiers: the key's base character.
    None,
    /// ⌘ held: some layouts map keys differently under ⌘ (e.g. digits on
    /// the AZERTY number row), and that is the character the shortcut means.
    Command,
}

/// One translation attempt: a layout and the modifier state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Translation {
    pub layout: LayoutKind,
    pub mods: TranslateMods,
}

const PLAN_COMMAND: [Translation; 2] = [
    Translation { layout: LayoutKind::AsciiCapable, mods: TranslateMods::Command },
    Translation { layout: LayoutKind::AsciiCapable, mods: TranslateMods::None },
];
const PLAN_CONTROL: [Translation; 1] = [Translation { layout: LayoutKind::AsciiCapable, mods: TranslateMods::None }];
const PLAN_PLAIN: [Translation; 1] = [Translation { layout: LayoutKind::Current, mods: TranslateMods::None }];

/// The translations to try, in order, for a key pressed with `mods`.
///
/// * ⌘ (with anything else): the ASCII-capable layout under ⌘, then its
///   base character — how macOS matches and draws menu shortcuts (⌘Z on
///   AZERTY is the key labelled Z; ⌘C on Russian is the key under С).
/// * ⌃ without ⌘: the ASCII-capable layout's base character (translating
///   with ⌃ held only yields control characters).
/// * otherwise (⌥ combos, special keys): the selected layout's base
///   character. ⌥ is shown as a glyph, so it is not applied (`⌥E`, not `⌥´`).
pub fn translation_plan(mods: KeyMods) -> &'static [Translation] {
    if mods.command {
        &PLAN_COMMAND
    } else if mods.control {
        &PLAN_CONTROL
    } else {
        &PLAN_PLAIN
    }
}

/// Turns a raw translated string into a key label: one to three printable
/// characters, letters upper-cased (`ф` → `Ф`, `z` → `Z`). `None` for
/// empty, whitespace, control characters and the private-use code points
/// macOS returns for function keys.
pub fn display_char(raw: &str) -> Option<String> {
    let n = raw.chars().count();
    if n == 0 || n > 3 {
        return None;
    }
    let mut out = String::new();
    for c in raw.chars() {
        if c.is_control() || c.is_whitespace() || ('\u{E000}'..='\u{F8FF}').contains(&c) {
            return None;
        }
        let mut up = c.to_uppercase();
        match (up.next(), up.next()) {
            // Only take single-character upper cases (`ß` stays `ß`).
            (Some(u), None) => out.push(u),
            _ => out.push(c),
        }
    }
    Some(out)
}

/// Source of the characters on keys (a keyboard layout).
pub trait KeyTranslator {
    /// The character(s) `keycode` produces with `how`, raw (not filtered),
    /// or `None` when the translation failed.
    fn translate(&mut self, keycode: u16, how: Translation) -> Option<String>;
}

/// The built-in US table only (no layout translation).
#[derive(Debug, Clone, Copy, Default)]
pub struct UsLayout;

impl KeyTranslator for UsLayout {
    fn translate(&mut self, _keycode: u16, _how: Translation) -> Option<String> {
        None
    }
}

/// The key part of a label (no modifier glyphs) for a key that may be
/// shown: named keys from the table, everything else from `layout` along
/// [`translation_plan`], falling back to the US table.
fn key_text(keycode: u16, mods: KeyMods, layout: &mut dyn KeyTranslator) -> Option<String> {
    let us = key_label(keycode);
    if is_named_key(keycode) {
        return us.map(|k| k.text.to_owned());
    }
    // Virtual key codes are 7-bit; anything else is junk.
    if keycode < 128 {
        for how in translation_plan(mods) {
            if let Some(text) = layout.translate(keycode, *how).as_deref().and_then(display_char) {
                return Some(text);
            }
        }
    }
    us.map(|k| k.text.to_owned())
}

/// The label to show for one key press, or `None` when it must not be
/// shown (plain typing, unknown key). US layout.
pub fn combo_label(keycode: u16, mods: KeyMods) -> Option<String> {
    combo_label_with(keycode, mods, &mut UsLayout)
}

/// [`combo_label`] with key characters from `layout`. Plain typing returns
/// `None` without consulting `layout`.
pub fn combo_label_with(keycode: u16, mods: KeyMods, layout: &mut dyn KeyTranslator) -> Option<String> {
    let special = key_label(keycode).is_some_and(|k| k.special);
    if !(special || mods.is_shortcut()) {
        return None;
    }
    let key = key_text(keycode, mods, layout)?;
    Some(format!("{}{}", mods.glyphs(), key))
}

/// Upper bound on cached translations before the cache starts over.
const CACHE_LIMIT: usize = 2048;

/// Translations cached per input source: an entry is keyed by the source's
/// id (e.g. `com.apple.keylayout.French`), the key code and the
/// [`Translation`], so switching layouts back and forth needs no
/// re-translation and an entry never leaks into another layout. A source
/// whose id is unknown is never cached.
#[derive(Debug, Default)]
pub struct LayoutCache {
    current: Option<String>,
    ascii: Option<String>,
    entries: HashMap<(String, u16, Translation), Option<String>>,
}

impl LayoutCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The input source ids now in use (after a layout change or at the
    /// start of a session).
    pub fn set_sources(&mut self, current: Option<String>, ascii: Option<String>) {
        self.current = current;
        self.ascii = ascii;
    }

    /// The id of the source a translation with `layout` uses.
    pub fn source_id(&self, layout: LayoutKind) -> Option<&str> {
        match layout {
            LayoutKind::Current => self.current.as_deref(),
            LayoutKind::AsciiCapable => self.ascii.as_deref(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The cached translation, or `translate()` stored under the current
    /// source id (failures included, so a failing key is not retried).
    pub fn get_or_translate(
        &mut self,
        keycode: u16,
        how: Translation,
        translate: impl FnOnce() -> Option<String>,
    ) -> Option<String> {
        let Some(id) = self.source_id(how.layout) else { return translate() };
        let key = (id.to_owned(), keycode, how);
        if let Some(hit) = self.entries.get(&key) {
            return hit.clone();
        }
        let value = translate();
        if self.entries.len() >= CACHE_LIMIT {
            self.entries.clear();
        }
        self.entries.insert(key, value.clone());
        value
    }
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
        self.key_down_with(keycode, mods, now, &mut UsLayout)
    }

    /// [`Self::key_down`] with key characters from `layout`.
    pub fn key_down_with(
        &mut self,
        keycode: u16,
        mods: KeyMods,
        now: f64,
        layout: &mut dyn KeyTranslator,
    ) -> Option<String> {
        let label = combo_label_with(keycode, mods, layout)?;
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

    /// A keyboard layout as an injected table: (keycode, translation) →
    /// raw output. Counts calls so tests can see what was translated.
    #[derive(Default)]
    struct Table {
        map: HashMap<(u16, LayoutKind, TranslateMods), &'static str>,
        calls: Vec<(u16, Translation)>,
    }

    impl Table {
        /// `current` is the selected layout's base characters, `ascii` the
        /// ASCII-capable layout's; `ascii_cmd` overrides under ⌘.
        fn new(
            current: &[(u16, &'static str)],
            ascii: &[(u16, &'static str)],
            ascii_cmd: &[(u16, &'static str)],
        ) -> Table {
            let mut map = HashMap::new();
            for (k, v) in current {
                map.insert((*k, LayoutKind::Current, TranslateMods::None), *v);
            }
            for (k, v) in ascii {
                map.insert((*k, LayoutKind::AsciiCapable, TranslateMods::None), *v);
                map.insert((*k, LayoutKind::AsciiCapable, TranslateMods::Command), *v);
            }
            for (k, v) in ascii_cmd {
                map.insert((*k, LayoutKind::AsciiCapable, TranslateMods::Command), *v);
            }
            Table { map, calls: Vec::new() }
        }

        /// A Latin layout: current and ASCII-capable are the same.
        fn latin(base: &[(u16, &'static str)], cmd: &[(u16, &'static str)]) -> Table {
            Table::new(base, base, cmd)
        }
    }

    impl KeyTranslator for Table {
        fn translate(&mut self, keycode: u16, how: Translation) -> Option<String> {
            self.calls.push((keycode, how));
            self.map.get(&(keycode, how.layout, how.mods)).map(|s| s.to_string())
        }
    }

    /// French AZERTY: A↔Q, Z↔W, M on the US `;` key, `&é"'(` on the number
    /// row, which gives digits under ⌘.
    fn azerty() -> Table {
        Table::latin(
            &[(0, "q"), (12, "a"), (13, "z"), (6, "w"), (41, "m"), (46, ","), (18, "&"), (19, "é"), (21, "\'")],
            &[(18, "1"), (19, "2"), (21, "4")],
        )
    }

    /// German QWERTZ: Y↔Z, umlauts, ß on the US `-` key, `^` dead key on `.
    fn qwertz() -> Table {
        Table::latin(&[(6, "y"), (16, "z"), (41, "ö"), (39, "ä"), (33, "ü"), (27, "ß"), (50, "^"), (44, "-")], &[])
    }

    const CTRL: KeyMods = KeyMods { control: true, option: false, shift: false, command: false };
    const OPT: KeyMods = KeyMods { control: false, option: true, shift: false, command: false };

    #[test]
    fn azerty_shortcuts_use_the_layout() {
        let mut t = azerty();
        // ⌘ + the key labelled W on AZERTY (US Z position) is ⌘W, not ⌘Z.
        assert_eq!(combo_label_with(6, CMD, &mut t).as_deref(), Some("⌘W"));
        assert_eq!(combo_label_with(13, CMD, &mut t).as_deref(), Some("⌘Z"));
        assert_eq!(combo_label_with(12, CMD, &mut t).as_deref(), Some("⌘A"));
        assert_eq!(combo_label_with(41, CMD, &mut t).as_deref(), Some("⌘M"));
        // Number row: digits under ⌘, the base character otherwise.
        assert_eq!(combo_label_with(18, CMD, &mut t).as_deref(), Some("⌘1"));
        let shift_cmd = KeyMods { shift: true, command: true, ..NONE };
        assert_eq!(combo_label_with(21, shift_cmd, &mut t).as_deref(), Some("⇧⌘4"));
        assert_eq!(combo_label_with(18, CTRL, &mut t).as_deref(), Some("⌃&"));
        assert_eq!(combo_label_with(19, OPT, &mut t).as_deref(), Some("⌥É"));
    }

    #[test]
    fn qwertz_shortcuts_use_the_layout() {
        let mut t = qwertz();
        assert_eq!(combo_label_with(6, CMD, &mut t).as_deref(), Some("⌘Y"));
        assert_eq!(combo_label_with(16, CMD, &mut t).as_deref(), Some("⌘Z"));
        assert_eq!(combo_label_with(41, CTRL, &mut t).as_deref(), Some("⌃Ö"));
        assert_eq!(combo_label_with(27, OPT, &mut t).as_deref(), Some("⌥ß"));
        // A dead key's own character (no dead-key state is kept).
        assert_eq!(combo_label_with(50, CMD, &mut t).as_deref(), Some("⌘^"));
        // Keys the table doesn't know fall back to US.
        assert_eq!(combo_label_with(8, CMD, &mut t).as_deref(), Some("⌘C"));
    }

    #[test]
    fn non_latin_layouts_use_the_ascii_layout_for_command_and_control() {
        // Russian: Cyrillic on the selected layout, ABC as the ASCII layout.
        let mut t = Table::new(&[(8, "с"), (0, "ф")], &[(8, "c"), (0, "a")], &[]);
        assert_eq!(combo_label_with(8, CMD, &mut t).as_deref(), Some("⌘C"));
        assert_eq!(combo_label_with(0, CTRL, &mut t).as_deref(), Some("⌃A"));
        assert_eq!(combo_label_with(0, OPT, &mut t).as_deref(), Some("⌥Ф"));
        assert!(t.calls.iter().filter(|(k, _)| *k == 8).all(|(_, h)| h.layout == LayoutKind::AsciiCapable));
    }

    #[test]
    fn plan_order() {
        let shift_cmd = KeyMods { shift: true, command: true, ..NONE };
        assert_eq!(translation_plan(shift_cmd), &PLAN_COMMAND);
        assert_eq!(translation_plan(KeyMods { control: true, command: true, ..NONE }), &PLAN_COMMAND);
        assert_eq!(translation_plan(CTRL), &PLAN_CONTROL);
        assert_eq!(translation_plan(OPT), &PLAN_PLAIN);
        assert_eq!(translation_plan(NONE), &PLAN_PLAIN);
    }

    #[test]
    fn named_keys_ignore_the_layout() {
        // A layout that maps everything to "x".
        struct Everything;
        impl KeyTranslator for Everything {
            fn translate(&mut self, _: u16, _: Translation) -> Option<String> {
                Some("x".into())
            }
        }
        for code in [36u16, 48, 49, 51, 53, 117, 122, 123, 126] {
            assert!(is_named_key(code), "{code}");
            let want = combo_label(code, CMD);
            assert_eq!(combo_label_with(code, CMD, &mut Everything), want, "{code}");
        }
        assert_eq!(combo_label_with(49, CMD, &mut Everything).as_deref(), Some("⌘␣"));
        assert_eq!(combo_label_with(36, NONE, &mut Everything).as_deref(), Some("↩"));
        assert!(!is_named_key(0));
        // ISO section key: unknown to the US table, known to the layout.
        assert_eq!(combo_label_with(10, CMD, &mut Everything).as_deref(), Some("⌘X"));
        assert_eq!(combo_label_with(10, CMD, &mut UsLayout), None);
        // Out-of-range codes are never translated.
        assert_eq!(combo_label_with(200, CMD, &mut Everything), None);
    }

    #[test]
    fn fallback_to_us_when_translation_is_unusable() {
        let mut t = Table::latin(&[(0, ""), (1, "\u{1}"), (2, " "), (3, "\u{F704}"), (4, "abcd"), (5, "\u{7f}")], &[]);
        for (code, us) in [(0u16, "⌘A"), (1, "⌘S"), (2, "⌘D"), (3, "⌘F"), (4, "⌘H"), (5, "⌘G")] {
            assert_eq!(combo_label_with(code, CMD, &mut t).as_deref(), Some(us), "{code}");
        }
        // ⌘ falls through to the base character before the US table.
        let mut t = Table::new(&[], &[(6, "w")], &[(6, "\u{17}")]);
        assert_eq!(combo_label_with(6, CMD, &mut t).as_deref(), Some("⌘W"));
    }

    #[test]
    fn typing_is_never_translated() {
        let mut t = azerty();
        for code in [0u16, 6, 12, 18, 49] {
            assert_eq!(combo_label_with(code, NONE, &mut t), None);
            assert_eq!(combo_label_with(code, SHIFT, &mut t), None);
        }
        assert!(t.calls.is_empty());
        let mut f = KeystrokeFeed::new();
        assert_eq!(f.key_down_with(0, NONE, 0.0, &mut t), None);
        assert!(t.calls.is_empty());
        assert_eq!(f.key_down_with(6, CMD, 0.1, &mut t).as_deref(), Some("⌘W"));
        assert_eq!(f.key_down_with(6, CMD, 0.2, &mut t).as_deref(), Some("⌘W ×2"));
    }

    #[test]
    fn display_char_rules() {
        assert_eq!(display_char("z").as_deref(), Some("Z"));
        assert_eq!(display_char("ф").as_deref(), Some("Ф"));
        assert_eq!(display_char("ß").as_deref(), Some("ß"));
        assert_eq!(display_char("é").as_deref(), Some("É"));
        assert_eq!(display_char("^").as_deref(), Some("^"));
        assert_eq!(display_char("1").as_deref(), Some("1"));
        for bad in ["", " ", "\t", "\r", "\u{0}", "\u{1b}", "\u{a0}", "\u{F700}", "abcd"] {
            assert_eq!(display_char(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn cache_is_keyed_by_input_source() {
        let how = PLAN_COMMAND[1];
        let mut c = LayoutCache::new();
        // Unknown source: never cached.
        assert_eq!(c.get_or_translate(0, how, || Some("a".into())).as_deref(), Some("a"));
        assert!(c.is_empty());
        c.set_sources(Some("com.apple.keylayout.French".into()), Some("com.apple.keylayout.French".into()));
        assert_eq!(c.get_or_translate(0, how, || Some("q".into())).as_deref(), Some("q"));
        assert_eq!(c.get_or_translate(0, how, || panic!("cached")).as_deref(), Some("q"));
        // Failures are cached too.
        assert_eq!(c.get_or_translate(1, how, || None), None);
        assert_eq!(c.get_or_translate(1, how, || panic!("cached")), None);
        // Another source has its own entries; switching back hits the cache.
        c.set_sources(Some("com.apple.keylayout.Russian".into()), Some("com.apple.keylayout.ABC".into()));
        assert_eq!(c.source_id(LayoutKind::Current), Some("com.apple.keylayout.Russian"));
        assert_eq!(c.get_or_translate(0, how, || Some("a".into())).as_deref(), Some("a"));
        let cur = Translation { layout: LayoutKind::Current, mods: TranslateMods::None };
        assert_eq!(c.get_or_translate(0, cur, || Some("ф".into())).as_deref(), Some("ф"));
        c.set_sources(Some("com.apple.keylayout.French".into()), Some("com.apple.keylayout.French".into()));
        assert_eq!(c.get_or_translate(0, how, || panic!("cached")).as_deref(), Some("q"));
        assert_eq!(c.len(), 4);
    }

    #[test]
    fn cache_is_bounded() {
        let mut c = LayoutCache::new();
        c.set_sources(Some("a".into()), Some("a".into()));
        let how = PLAN_PLAIN[0];
        for i in 0..(CACHE_LIMIT + 10) {
            let _ = c.get_or_translate((i % 128) as u16, Translation { layout: how.layout, mods: how.mods }, || None);
            if i % 128 == 127 {
                c.set_sources(Some(format!("s{i}")), Some("a".into()));
            }
        }
        assert!(c.len() <= CACHE_LIMIT);
    }

    #[test]
    fn position_index_round_trip() {
        for p in HudPosition::ALL {
            assert_eq!(HudPosition::from_index(p.index()), p);
        }
        assert_eq!(HudPosition::from_index(99), HudPosition::BottomCenter);
    }
}
