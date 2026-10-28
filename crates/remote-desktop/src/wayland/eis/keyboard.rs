//! Fallback for EIS compositors without ei_text. Never assume a US layout or
//! send a keysym as an evdev scancode. Unsupported text fails before any effect.
use crate::Result;
use std::{collections::VecDeque, fs::File, os::unix::fs::FileExt};
use xkbcommon::xkb;

pub(super) struct Map {
    keymap: xkb::Keymap,
    modifiers: (u32, u32, u32, u32),
    base_modifiers: (u32, u32, u32, u32),
    own: xkb::State,
    pressed: Vec<u32>,
    expected: VecDeque<(u32, u32, u32, u32)>,
}
pub(super) enum TextError {
    Unmappable,
    Invalid(String),
}
impl TextError {
    pub(super) fn message(self) -> String {
        match self {
            Self::Unmappable => {
                "当前桌面无法用现有键盘布局输入这段文字，且没有可用的文字粘贴通道；本次输入未执行"
                    .into()
            }
            Self::Invalid(message) => message,
        }
    }
}
impl Map {
    pub(super) fn read(keymap: &reis::event::Keymap) -> Result<Self> {
        if keymap.type_ != reis::ei::keyboard::KeymapType::Xkb
            || keymap.size == 0
            || keymap.size > 1024 * 1024
        {
            return Err("Wayland 键盘布局类型或大小无效".into());
        }
        let file = File::from(
            keymap
                .fd
                .try_clone()
                .map_err(|_| "无法读取已授权的键盘布局")?,
        );
        let meta = file.metadata().map_err(|_| "无法读取键盘布局信息")?;
        // Do not read pipes/sockets (which can block the actor) or mmap a short
        // compositor FD (which can SIGBUS). read_at leaves the shared offset alone.
        if !meta.is_file() || meta.len() < u64::from(keymap.size) {
            return Err("Wayland 键盘布局描述符无效".into());
        }
        let mut bytes = vec![0; keymap.size as usize];
        file.read_exact_at(&mut bytes, 0)
            .map_err(|_| "Wayland 键盘布局读取失败")?;
        if bytes.last() == Some(&0) {
            bytes.pop();
        }
        if bytes.contains(&0) {
            return Err("Wayland 键盘布局包含无效字符".into());
        }
        let text = String::from_utf8(bytes).map_err(|_| "Wayland 键盘布局编码无效")?;
        Self::parse(text)
    }
    fn parse(text: String) -> Result<Self> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_string(
            &context,
            text,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::COMPILE_NO_FLAGS,
        )
        .ok_or("Wayland 键盘布局无法解析")?;
        if keymap.max_keycode().raw() > 4096
            || keymap.min_keycode().raw() < 8
            || keymap.num_layouts() > 32
        {
            return Err("Wayland 键盘布局范围无效".into());
        }
        let own = xkb::State::new(&keymap);
        Ok(Self {
            keymap,
            modifiers: (0, 0, 0, 0),
            base_modifiers: (0, 0, 0, 0),
            own,
            pressed: Vec::new(),
            expected: VecDeque::new(),
        })
    }
    pub(super) fn modifiers(
        &mut self,
        depressed: u32,
        latched: u32,
        locked: u32,
        group: u32,
    ) -> bool {
        let incoming = (depressed, latched, locked, group);
        // Replies to our own modifier transitions are ordered before sync.done.
        // Accept coalesced replies only when their exact state was predicted.
        if let Some(at) = self.expected.iter().position(|v| *v == incoming) {
            self.expected.drain(..=at);
            return true;
        }
        if incoming == self.modifiers {
            return true;
        }
        self.expected.clear();
        self.modifiers = incoming;
        self.rebase();
        false
    }
    fn rebase(&mut self) {
        let (depressed, latched, locked, group) = self.modifiers;
        let groups = self.keymap.num_layouts().max(1);
        self.base_modifiers = (
            depressed & !self.own.serialize_mods(xkb::STATE_MODS_DEPRESSED),
            latched & !self.own.serialize_mods(xkb::STATE_MODS_LATCHED),
            locked ^ self.own.serialize_mods(xkb::STATE_MODS_LOCKED),
            (group % groups + groups
                - self.own.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE) % groups)
                % groups,
        );
    }
    /// Independent state for batch planning. xkb_state_ref aliases the mutable
    /// object, so it must not be used as a "clone". Rebuild owned key counts in
    /// press order, then preserve authoritative locks/layout in the base masks.
    /// Slave update_mask and master update_key stay on separate XKB objects.
    pub(super) fn fork(&self) -> Self {
        let mut own = xkb::State::new(&self.keymap);
        for code in &self.pressed {
            own.update_key(xkb::Keycode::new(*code + 8), xkb::KeyDirection::Down);
        }
        let mut map = Self {
            keymap: self.keymap.clone(),
            modifiers: self.modifiers,
            base_modifiers: (0, 0, 0, 0),
            own,
            pressed: self.pressed.clone(),
            expected: VecDeque::new(),
        };
        map.rebase();
        map
    }
    pub(super) fn note_key(&mut self, evdev: u32, down: bool) {
        if down {
            if self.pressed.contains(&evdev) {
                return;
            }
            self.pressed.push(evdev);
        } else {
            let Some(at) = self.pressed.iter().position(|code| *code == evdev) else {
                return;
            };
            self.pressed.remove(at);
        }
        self.own.update_key(
            xkb::Keycode::new(evdev + 8),
            if down {
                xkb::KeyDirection::Down
            } else {
                xkb::KeyDirection::Up
            },
        );
        let base = self.base_modifiers;
        let next = (
            base.0 | self.own.serialize_mods(xkb::STATE_MODS_DEPRESSED),
            base.1 | self.own.serialize_mods(xkb::STATE_MODS_LATCHED),
            base.2 ^ self.own.serialize_mods(xkb::STATE_MODS_LOCKED),
            base.3
                .wrapping_add(self.own.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE))
                % self.keymap.num_layouts().max(1),
        );
        if next != self.modifiers {
            self.expected.push_back(next);
            self.modifiers = next;
        }
    }
    pub(super) fn synchronized(&mut self) {
        self.expected.clear();
    }
    pub(super) fn reset(&mut self) {
        self.own = xkb::State::new(&self.keymap);
        self.pressed.clear();
        self.expected.clear();
        self.base_modifiers = (0, 0, 0, 0);
        self.modifiers = self.base_modifiers;
    }
    fn codes(&self) -> std::ops::RangeInclusive<u32> {
        self.keymap.min_keycode().raw()..=self.keymap.max_keycode().raw().min(0x2ff + 8)
    }
    pub(super) fn key(&self, sym: i32) -> Result<u32> {
        // A shortcut's modifiers are explicit Input::Key events. Resolve its
        // base key in the current group; do not silently inject Shift/AltGr.
        self.base(sym as u32)
            .map(|code| code - 8)
            .ok_or_else(|| "当前键盘布局没有此按键，请切换布局后重试".into())
    }
    fn base(&self, sym: u32) -> Option<u32> {
        self.codes().find(|code| {
            matches_symbol(
                self.keymap
                    .key_get_syms_by_level(xkb::Keycode::new(*code), self.modifiers.3, 0),
                sym,
            )
        })
    }
    #[cfg(test)]
    pub(super) fn text(&self, text: &str) -> Result<Vec<(u32, bool)>> {
        self.plan_text(text).map_err(TextError::message)
    }
    pub(super) fn plan_text(&self, text: &str) -> std::result::Result<Vec<(u32, bool)>, TextError> {
        if self.modifiers.0 != 0 || self.modifiers.1 != 0 {
            return Err(TextError::Invalid(
                "电脑仍有修饰键按下，请松开后再输入文字".into(),
            ));
        }
        let modifier = |name: &str, sym: u32| {
            self.keymap
                .key_by_name(name)
                .filter(|code| {
                    self.keymap
                        .key_get_syms_by_level(*code, self.modifiers.3, 0)
                        == [xkb::Keysym::new(sym)]
                })
                .map(|code| code.raw())
                .or_else(|| self.base(sym))
        };
        let shift = modifier("LFSH", xkb::Keysym::Shift_L.raw());
        let level3 = modifier("RALT", xkb::Keysym::ISO_Level3_Shift.raw());
        let mut options = vec![Vec::new()];
        if let Some(code) = shift {
            options.push(vec![code]);
        }
        if let Some(code) = level3 {
            options.push(vec![code]);
        }
        if let (Some(a), Some(b)) = (shift, level3) {
            options.push(vec![a, b]);
        }
        let options: Vec<_> = options
            .into_iter()
            .map(|keys| {
                let mut physical = xkb::State::new(&self.keymap);
                for key in &keys {
                    physical.update_key(xkb::Keycode::new(*key), xkb::KeyDirection::Down);
                }
                let mask = physical.serialize_mods(xkb::STATE_MODS_DEPRESSED);
                let mut state = xkb::State::new(&self.keymap);
                state.update_mask(mask, 0, self.modifiers.2, 0, 0, self.modifiers.3);
                (keys, state)
            })
            .collect();
        let mut out = Vec::new();
        for c in text.chars() {
            let sym = super::input::unicode(c) as u32;
            let found = options
                .iter()
                .find_map(|(mods, state)| {
                    self.codes()
                        .find(|code| {
                            matches_symbol(state.key_get_syms(xkb::Keycode::new(*code)), sym)
                        })
                        .map(|code| (mods, code))
                })
                .ok_or(TextError::Unmappable)?;
            for code in found.0 {
                out.push((code - 8, true));
            }
            out.push((found.1 - 8, true));
            out.push((found.1 - 8, false));
            for code in found.0.iter().rev() {
                out.push((code - 8, false));
            }
            if out.len() > 32768 {
                return Err(TextError::Invalid("单次文字输入过多".into()));
            }
        }
        Ok(out)
    }
}

fn matches_symbol(actual: &[xkb::Keysym], requested: u32) -> bool {
    if actual == [xkb::Keysym::new(requested)] {
        return true;
    }
    // Current XKB layouts may encode a printable character with a named keysym
    // (e.g. Cyrillic_ef), while literal input uses its direct Unicode encoding.
    // Match their character, preserving exact semantics for named control keys
    // and rejecting multi-symbol keys instead of synthesizing a compose sequence.
    let [symbol] = actual else { return false };
    (0x01000100..=0x0110ffff).contains(&requested)
        && xkb::keysym_to_utf32(*symbol) == (requested & 0x00ffffff)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn map(layout: &str) -> Map {
        let context = xkb::Context::new(0);
        let keymap =
            xkb::Keymap::new_from_names(&context, "evdev", "pc105", layout, "", None, 0).unwrap();
        Map::parse(keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1)).unwrap()
    }
    #[test]
    fn non_latin_text_uses_the_layouts_actual_symbols_with_shift_and_caps_lock() {
        for (layout, text, letter, code) in
            [("ru", "Привет Ёё", 'ф', 30), ("gr", "Ελλαδα Ωω", 'α', 30)]
        {
            for caps in [false, true] {
                let mut map = map(layout);
                let mut receiver = xkb::State::new(&map.keymap);
                if caps {
                    let mask = 1 << map.keymap.mod_get_index(xkb::MOD_NAME_CAPS);
                    map.modifiers(0, 0, mask, 0);
                    let key = xkb::Keycode::new(map.key(0xffe5).unwrap() + 8);
                    receiver.update_key(key, xkb::KeyDirection::Down);
                    receiver.update_key(key, xkb::KeyDirection::Up);
                }
                let mut typed = String::new();
                for (code, down) in map.text(text).unwrap() {
                    let key = xkb::Keycode::new(code + 8);
                    receiver.update_key(
                        key,
                        if down {
                            xkb::KeyDirection::Down
                        } else {
                            xkb::KeyDirection::Up
                        },
                    );
                    if down {
                        typed.push_str(&receiver.key_get_utf8(key));
                    }
                }
                assert_eq!(typed, text, "{layout}, caps={caps}");
                assert_eq!(map.key(super::super::input::unicode(letter)).unwrap(), code);
                assert_eq!(map.key(0xff0d).unwrap(), 28);
                assert!(
                    map.key('v' as i32).is_err(),
                    "a Latin shortcut must not guess a different layout"
                );
            }
        }
    }
    #[test]
    fn keycodes_follow_server_layout_instead_of_ascii_or_keysym_numbers() {
        let us = map("us");
        let de = map("de");
        assert_eq!(us.key('y' as i32).unwrap(), 21);
        assert_eq!(de.key('y' as i32).unwrap(), 44);
        assert_eq!(de.key('z' as i32).unwrap(), 21);
        assert_eq!(us.key(0xffe3).unwrap(), 29);
        assert_eq!(us.key(0xff0d).unwrap(), 28);
    }
    #[test]
    fn text_synthesizes_shift_altgr_and_respects_caps_lock() {
        let mut us = map("us");
        assert_eq!(
            us.text("A").unwrap(),
            vec![(42, true), (30, true), (30, false), (42, false)]
        );
        assert_eq!(
            us.text("\n\t").unwrap(),
            vec![(28, true), (28, false), (15, true), (15, false)]
        );
        let de = map("de");
        assert_eq!(
            de.text("@").unwrap(),
            vec![(100, true), (16, true), (16, false), (100, false)]
        );
        let caps = 1 << us.keymap.mod_get_index(xkb::MOD_NAME_CAPS);
        us.modifiers(0, 0, caps, 0);
        assert_eq!(us.text("A").unwrap(), vec![(30, true), (30, false)]);
        assert_eq!(
            us.text("a").unwrap(),
            vec![(42, true), (30, true), (30, false), (42, false)]
        );
    }
    #[test]
    fn full_text_validation_rejects_unmappable_suffix_or_held_modifiers() {
        let mut us = map("us");
        assert!(us.text("abc中").is_err());
        assert!(us.text("🟢").is_err());
        us.modifiers(4, 0, 0, 0);
        assert!(us.text("a").is_err());
    }
    #[test]
    fn current_group_selects_the_actual_shortcut_key() {
        let mut map = map("us,de");
        assert_eq!(map.key('y' as i32).unwrap(), 21);
        map.modifiers(0, 0, 0, 1);
        assert_eq!(map.key('y' as i32).unwrap(), 44);
    }
    #[test]
    fn own_modifier_feedback_is_distinct_from_external_layout_or_lock_changes() {
        let mut map = map("us");
        map.note_key(42, true);
        map.note_key(30, true);
        map.note_key(30, false);
        map.note_key(42, false);
        assert!(map.modifiers(1, 0, 0, 0));
        assert!(map.modifiers(0, 0, 0, 0));
        map.synchronized();
        assert!(!map.modifiers(0, 0, 0, 1));
        map.reset();
        map.note_key(42, true);
        map.note_key(54, true);
        map.note_key(42, false);
        assert!(map.modifiers(1, 0, 0, 0));
        assert!(map.text("a").is_err());
        map.note_key(54, false);
        assert!(map.modifiers(0, 0, 0, 0));
        assert!(map.text("a").is_ok());
    }
    #[test]
    fn keymap_fd_cannot_block_or_map_past_a_short_file() {
        use std::os::fd::OwnedFd;
        let (fd, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        assert!(
            Map::read(&reis::event::Keymap {
                fd: OwnedFd::from(fd),
                size: 10,
                type_: reis::ei::keyboard::KeymapType::Xkb
            })
            .is_err()
        );
        let file = tempfile::tempfile().unwrap();
        assert!(
            Map::read(&reis::event::Keymap {
                fd: file.into(),
                size: 4096,
                type_: reis::ei::keyboard::KeymapType::Xkb
            })
            .is_err()
        );
    }
    #[test]
    fn fork_keeps_lock_state_and_key_counts_without_mutating_the_live_map() {
        let mut live = map("us");
        live.note_key(58, true);
        live.note_key(58, false);
        live.note_key(42, true);
        live.note_key(54, true);
        let mut planned = live.fork();
        planned.note_key(42, false);
        assert!(planned.text("a").is_err(), "the right shift is still held");
        planned.note_key(54, false);
        assert_eq!(
            planned.text("A").unwrap(),
            vec![(30, true), (30, false)],
            "the inherited Caps Lock was lost"
        );
        planned.note_key(58, true);
        planned.note_key(58, false);
        assert_eq!(planned.text("a").unwrap(), vec![(30, true), (30, false)]);
        assert_eq!(live.modifiers.0, 1);
        assert_ne!(live.modifiers.2, 0);
        assert_eq!(live.pressed, [42, 54]);
    }
}
