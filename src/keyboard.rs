use std::io::{self, Write};
use std::os::fd::AsFd;

use rustix::fs::{MemfdFlags, memfd_create};
use tracing::info;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;
use xkbcommon::xkb;

/// Generate an XKB keymap string from the given layout parameters.
pub fn generate_keymap(layout: &str, variant: &str, options: &str) -> String {
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);

    let opts: Option<String> = if options.is_empty() {
        None
    } else {
        Some(options.to_owned())
    };

    let keymap = xkb::Keymap::new_from_names(
        &context,
        "", // rules (default evdev)
        "", // model (default pc105)
        layout,
        variant,
        opts,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .expect("failed to compile XKB keymap");

    keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1)
}

/// Send an XKB keymap to the compositor via a shared-memory file descriptor.
pub fn send_keymap(vkbd: &ZwpVirtualKeyboardV1, keymap_str: &str) -> io::Result<()> {
    let keymap_bytes = keymap_str.as_bytes();
    let size = keymap_bytes.len();

    let owned_fd =
        memfd_create(c"evbridge-xkb-keymap", MemfdFlags::CLOEXEC).map_err(io::Error::other)?;

    let mut file = std::fs::File::from(owned_fd);
    file.write_all(keymap_bytes)?;

    // format 1 = XKB_V1
    vkbd.keymap(1, file.as_fd(), size as u32);

    info!(size, "sent XKB keymap to compositor");
    Ok(())
}

/// XKB state tracker for a virtual keyboard.
/// Tracks modifier state and sends modifiers() to the compositor when it changes.
pub struct KeyboardState {
    state: xkb::State,
}

impl KeyboardState {
    pub fn new(keymap_str: &str) -> Self {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_string(
            &context,
            keymap_str.to_string(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("failed to compile XKB keymap for state tracking");

        let state = xkb::State::new(&keymap);
        Self { state }
    }

    /// Process a key event and send modifiers to compositor if state changed.
    /// `scancode` is the evdev scancode; XKB expects scancode + 8.
    pub fn forward_key(
        &mut self,
        vkbd: &ZwpVirtualKeyboardV1,
        time_ms: u32,
        scancode: u32,
        pressed: bool,
    ) {
        let wayland_state: u32 = if pressed { 1 } else { 0 };
        vkbd.key(time_ms, scancode, wayland_state);

        // XKB keycodes are evdev scancodes + 8
        let xkb_keycode = scancode + 8;
        let direction = if pressed {
            xkb::KeyDirection::Down
        } else {
            xkb::KeyDirection::Up
        };

        let changed = self.state.update_key(xkb_keycode.into(), direction);

        if changed != 0 {
            let mods_depressed = self.state.serialize_mods(xkb::STATE_MODS_DEPRESSED);
            let mods_latched = self.state.serialize_mods(xkb::STATE_MODS_LATCHED);
            let mods_locked = self.state.serialize_mods(xkb::STATE_MODS_LOCKED);
            let group = self.state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE);

            vkbd.modifiers(mods_depressed, mods_latched, mods_locked, group);
        }
    }
}
