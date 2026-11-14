//! Parsing, formatting and validation of global keyboard shortcuts.
//!
//! Shortcuts are stored in settings as human strings — either symbol form
//! (`"⌥⇧4"`) or word form (`"alt+shift+4"`, `"cmd+opt+R"`) — and converted
//! here into a [`Hotkey`] holding the Carbon virtual key code and modifier
//! mask that `RegisterEventHotKey` expects. Key codes are for the ANSI
//! layout position (as Carbon hotkeys are positional).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// Modifier keys as a small bitset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const CONTROL: Modifiers = Modifiers(1);
    pub const OPTION: Modifiers = Modifiers(2);
    pub const SHIFT: Modifiers = Modifiers(4);
    pub const COMMAND: Modifiers = Modifiers(8);

    pub const fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }
    pub const fn union(self, other: Modifiers) -> Modifiers {
        Modifiers(self.0 | other.0)
    }
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Carbon `EventModifiers` mask (`cmdKey`, `shiftKey`, `optionKey`,
    /// `controlKey` from `Events.h`).
    pub fn carbon_mask(self) -> u32 {
        let mut m = 0;
        if self.contains(Self::COMMAND) {
            m |= 0x0100;
        }
        if self.contains(Self::SHIFT) {
            m |= 0x0200;
        }
        if self.contains(Self::OPTION) {
            m |= 0x0800;
        }
        if self.contains(Self::CONTROL) {
            m |= 0x1000;
        }
        m
    }

    /// Symbols in Apple's canonical order: ⌃ ⌥ ⇧ ⌘.
    pub fn symbols(self) -> String {
        let mut s = String::new();
        for (m, sym) in [(Self::CONTROL, '⌃'), (Self::OPTION, '⌥'), (Self::SHIFT, '⇧'), (Self::COMMAND, '⌘')] {
            if self.contains(m) {
                s.push(sym);
            }
        }
        s
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Modifiers;
    fn bitor(self, rhs: Modifiers) -> Modifiers {
        self.union(rhs)
    }
}

/// A physical key, identified by its Carbon virtual key code and a display
/// label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: u16,
    pub label: &'static str,
}

/// (label, aliases, carbon key code). Labels are what we display.
const KEYS: &[(&str, &[&str], u16)] = &[
    ("A", &[], 0x00),
    ("S", &[], 0x01),
    ("D", &[], 0x02),
    ("F", &[], 0x03),
    ("H", &[], 0x04),
    ("G", &[], 0x05),
    ("Z", &[], 0x06),
    ("X", &[], 0x07),
    ("C", &[], 0x08),
    ("V", &[], 0x09),
    ("B", &[], 0x0B),
    ("Q", &[], 0x0C),
    ("W", &[], 0x0D),
    ("E", &[], 0x0E),
    ("R", &[], 0x0F),
    ("Y", &[], 0x10),
    ("T", &[], 0x11),
    ("1", &[], 0x12),
    ("2", &[], 0x13),
    ("3", &[], 0x14),
    ("4", &[], 0x15),
    ("6", &[], 0x16),
    ("5", &[], 0x17),
    ("=", &["equal"], 0x18),
    ("9", &[], 0x19),
    ("7", &[], 0x1A),
    ("-", &["minus"], 0x1B),
    ("8", &[], 0x1C),
    ("0", &[], 0x1D),
    ("]", &["rightbracket"], 0x1E),
    ("O", &[], 0x1F),
    ("U", &[], 0x20),
    ("[", &["leftbracket"], 0x21),
    ("I", &[], 0x22),
    ("P", &[], 0x23),
    ("↩", &["return", "enter"], 0x24),
    ("L", &[], 0x25),
    ("J", &[], 0x26),
    ("'", &["quote"], 0x27),
    ("K", &[], 0x28),
    (";", &["semicolon"], 0x29),
    ("\\", &["backslash"], 0x2A),
    (",", &["comma"], 0x2B),
    ("/", &["slash"], 0x2C),
    ("N", &[], 0x2D),
    ("M", &[], 0x2E),
    (".", &["period"], 0x2F),
    ("⇥", &["tab"], 0x30),
    ("Space", &["space", "␣"], 0x31),
    ("`", &["grave", "backtick"], 0x32),
    ("⌫", &["delete", "backspace"], 0x33),
    ("⎋", &["escape", "esc"], 0x35),
    ("F1", &[], 0x7A),
    ("F2", &[], 0x78),
    ("F3", &[], 0x63),
    ("F4", &[], 0x76),
    ("F5", &[], 0x60),
    ("F6", &[], 0x61),
    ("F7", &[], 0x62),
    ("F8", &[], 0x64),
    ("F9", &[], 0x65),
    ("F10", &[], 0x6D),
    ("F11", &[], 0x67),
    ("F12", &[], 0x6F),
    ("←", &["left"], 0x7B),
    ("→", &["right"], 0x7C),
    ("↓", &["down"], 0x7D),
    ("↑", &["up"], 0x7E),
];

impl Key {
    /// Look up a key by label or alias (case-insensitive).
    pub fn from_name(name: &str) -> Option<Key> {
        let lower = name.to_lowercase();
        KEYS.iter()
            .find(|(label, aliases, _)| label.to_lowercase() == lower || aliases.contains(&lower.as_str()))
            .map(|&(label, _, code)| Key { code, label })
    }

    /// Look up a key by Carbon key code (used when recording a shortcut).
    pub fn from_code(code: u16) -> Option<Key> {
        KEYS.iter().find(|(_, _, c)| *c == code).map(|&(label, _, code)| Key { code, label })
    }
}

/// A complete shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    pub key: Key,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HotkeyError {
    #[error("shortcut is empty")]
    Empty,
    #[error("unknown key `{0}`")]
    UnknownKey(String),
    #[error("unknown modifier `{0}`")]
    UnknownModifier(String),
    #[error("shortcut has no key, only modifiers")]
    MissingKey,
    #[error("shortcut has more than one key")]
    MultipleKeys,
    #[error("global shortcuts need at least one of ⌃ ⌥ ⌘")]
    NeedsModifier,
}

fn modifier_from_word(w: &str) -> Option<Modifiers> {
    match w.to_lowercase().as_str() {
        "ctrl" | "control" | "⌃" | "^" => Some(Modifiers::CONTROL),
        "alt" | "opt" | "option" | "⌥" => Some(Modifiers::OPTION),
        "shift" | "⇧" => Some(Modifiers::SHIFT),
        "cmd" | "command" | "super" | "meta" | "⌘" => Some(Modifiers::COMMAND),
        _ => None,
    }
}

fn modifier_from_symbol(c: char) -> Option<Modifiers> {
    match c {
        '⌃' => Some(Modifiers::CONTROL),
        '⌥' => Some(Modifiers::OPTION),
        '⇧' => Some(Modifiers::SHIFT),
        '⌘' => Some(Modifiers::COMMAND),
        _ => None,
    }
}

impl FromStr for Hotkey {
    type Err = HotkeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(HotkeyError::Empty);
        }
        let mut mods = Modifiers::NONE;
        let mut key: Option<Key> = None;

        let set_key = |k: Key, key: &mut Option<Key>| -> Result<(), HotkeyError> {
            if key.replace(k).is_some() {
                return Err(HotkeyError::MultipleKeys);
            }
            Ok(())
        };

        // Word form: "alt+shift+4" or "cmd-opt-r". A trailing '-' is the minus
        // key itself ("⌘-"), so dash-separated form requires no trailing dash.
        let sep = if s.contains('+') {
            Some('+')
        } else if s.len() > 1 && s.contains('-') && !s.ends_with('-') {
            Some('-')
        } else {
            None
        };
        if let Some(sep) = sep {
            let parts: Vec<&str> = s.split(sep).map(str::trim).filter(|p| !p.is_empty()).collect();
            for (i, p) in parts.iter().enumerate() {
                let is_last = i + 1 == parts.len();
                if let Some(m) = modifier_from_word(p) {
                    mods = mods | m;
                } else if p.chars().all(|c| modifier_from_symbol(c).is_some()) {
                    for c in p.chars() {
                        mods = mods | modifier_from_symbol(c).unwrap_or_default();
                    }
                } else if let Some(k) = Key::from_name(p) {
                    set_key(k, &mut key)?;
                } else if is_last {
                    return Err(HotkeyError::UnknownKey((*p).to_string()));
                } else {
                    return Err(HotkeyError::UnknownModifier((*p).to_string()));
                }
            }
        } else {
            // Symbol form: leading modifier glyphs then the key name.
            let mut rest = s;
            while let Some(c) = rest.chars().next() {
                match modifier_from_symbol(c) {
                    Some(m) => {
                        mods = mods | m;
                        rest = &rest[c.len_utf8()..];
                    }
                    None => break,
                }
            }
            let rest = rest.trim();
            if !rest.is_empty() {
                let k = Key::from_name(rest).ok_or_else(|| HotkeyError::UnknownKey(rest.to_string()))?;
                set_key(k, &mut key)?;
            }
        }

        let key = key.ok_or(HotkeyError::MissingKey)?;
        // Shift alone would steal ordinary typing.
        if !(mods.contains(Modifiers::CONTROL) || mods.contains(Modifiers::OPTION) || mods.contains(Modifiers::COMMAND))
        {
            return Err(HotkeyError::NeedsModifier);
        }
        Ok(Hotkey { modifiers: mods, key })
    }
}

impl fmt::Display for Hotkey {
    /// Canonical symbol form, e.g. `⌥⇧4`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.modifiers.symbols(), self.key.label)
    }
}

impl Serialize for Hotkey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Hotkey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Shortcuts macOS reserves for its own screenshot tool. Binding these would
/// silently fail or fight the system.
pub fn system_reserved() -> Vec<Hotkey> {
    ["⇧⌘3", "⇧⌘4", "⇧⌘5", "⇧⌘6", "⌃⇧⌘3", "⌃⇧⌘4"].iter().filter_map(|s| s.parse().ok()).collect()
}

/// A problem with a set of bindings, for the Shortcuts tab to display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// Two of our actions share a shortcut (indices into the input slice).
    Duplicate(usize, usize),
    /// The binding at this index collides with a macOS system shortcut.
    System(usize),
}

/// Find duplicate or system-reserved bindings. `None` entries are unbound.
pub fn find_conflicts(bindings: &[Option<Hotkey>]) -> Vec<Conflict> {
    let reserved = system_reserved();
    let mut out = Vec::new();
    for (i, a) in bindings.iter().enumerate() {
        let Some(a) = a else { continue };
        if reserved.contains(a) {
            out.push(Conflict::System(i));
        }
        for (j, b) in bindings.iter().enumerate().skip(i + 1) {
            if Some(*a) == *b {
                out.push(Conflict::Duplicate(i, j));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hk(s: &str) -> Hotkey {
        s.parse().unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn symbol_and_word_forms_agree() {
        assert_eq!(hk("⌥⇧4"), hk("alt+shift+4"));
        assert_eq!(hk("⇧⌥4"), hk("Option + Shift + 4"));
        assert_eq!(hk("cmd-opt-r"), hk("⌥⌘R"));
        assert_eq!(hk("ctrl+shift+space"), hk("⌃⇧Space"));
    }

    #[test]
    fn display_is_canonical_order() {
        assert_eq!(hk("shift+cmd+ctrl+alt+a").to_string(), "⌃⌥⇧⌘A");
        assert_eq!(hk("alt+shift+r").to_string(), "⌥⇧R");
        assert_eq!(hk("⌥⇧left").to_string(), "⌥⇧←");
    }

    #[test]
    fn carbon_codes_and_masks() {
        let h = hk("⌥⇧4");
        assert_eq!(h.key.code, 0x15);
        assert_eq!(h.modifiers.carbon_mask(), 0x0800 | 0x0200);
        assert_eq!(hk("⌘R").key.code, 0x0F);
        assert_eq!(hk("⌃F12").modifiers.carbon_mask(), 0x1000);
        assert_eq!(Key::from_code(0x31).unwrap().label, "Space");
    }

    #[test]
    fn minus_key_parses() {
        assert_eq!(hk("cmd+-").key.label, "-");
        assert_eq!(hk("⌘-").key.label, "-");
    }

    #[test]
    fn errors() {
        assert_eq!("".parse::<Hotkey>(), Err(HotkeyError::Empty));
        assert_eq!("⌥⇧".parse::<Hotkey>(), Err(HotkeyError::MissingKey));
        assert_eq!("shift+a".parse::<Hotkey>(), Err(HotkeyError::NeedsModifier));
        assert_eq!("a".parse::<Hotkey>(), Err(HotkeyError::NeedsModifier));
        assert_eq!("alt+a+b".parse::<Hotkey>(), Err(HotkeyError::MultipleKeys));
        assert!(matches!("alt+ü".parse::<Hotkey>(), Err(HotkeyError::UnknownKey(_))));
        assert!(matches!("hyper+a".parse::<Hotkey>(), Err(HotkeyError::UnknownModifier(_))));
    }

    #[test]
    fn serde_round_trip() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct W {
            k: Hotkey,
        }
        let w = W { k: hk("alt+shift+r") };
        let s = toml::to_string(&w).unwrap();
        assert!(s.contains("⌥⇧R"));
        assert_eq!(toml::from_str::<W>(&s).unwrap(), w);
        assert!(toml::from_str::<W>("k = \"nope\"").is_err());
    }

    #[test]
    fn conflicts() {
        let b = vec![Some(hk("⌥⇧4")), Some(hk("⇧⌘4")), None, Some(hk("alt+shift+4"))];
        let c = find_conflicts(&b);
        assert!(c.contains(&Conflict::System(1)));
        assert!(c.contains(&Conflict::Duplicate(0, 3)));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn all_default_shortcuts_parse_without_conflict() {
        let b: Vec<_> = crate::Action::ALL.iter().map(|a| Some(hk(a.default_shortcut()))).collect();
        assert!(find_conflicts(&b).is_empty());
    }
}
