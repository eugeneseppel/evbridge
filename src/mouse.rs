use std::collections::BTreeSet;

use wayland_client::protocol::wl_pointer::{Axis, ButtonState};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

/// Accumulated mouse events within a single frame (between SYN_REPORTs).
/// Flushed atomically to the Wayland virtual pointer.
#[derive(Debug)]
pub struct MouseFrame {
    dx: f64,
    dy: f64,
    buttons: Vec<(u32, bool)>,
    pressed_buttons: BTreeSet<u32>,
    scroll_vert: f64,
    scroll_horiz: f64,
    abs: Option<(u32, u32)>,
    has_data: bool,
}

impl Default for MouseFrame {
    fn default() -> Self {
        Self::new()
    }
}

impl MouseFrame {
    pub fn new() -> Self {
        Self {
            dx: 0.0,
            dy: 0.0,
            buttons: Vec::new(),
            pressed_buttons: BTreeSet::new(),
            scroll_vert: 0.0,
            scroll_horiz: 0.0,
            abs: None,
            has_data: false,
        }
    }

    pub fn accumulate_rel_x(&mut self, value: i32) {
        self.dx += value as f64;
        self.has_data = true;
    }

    pub fn accumulate_rel_y(&mut self, value: i32) {
        self.dy += value as f64;
        self.has_data = true;
    }

    pub fn accumulate_button(&mut self, code: u32, value: i32) {
        // EV_KEY uses 0 for release, 1 for press, and 2 for repeat. Pointer
        // repeats must not become duplicate presses: wlroots tracks presses as
        // a count, so one physical release would otherwise leave the button
        // logically held. Also discard duplicate transitions defensively.
        let pressed = match value {
            0 => false,
            1 => true,
            _ => return,
        };
        let changed = if pressed {
            self.pressed_buttons.insert(code)
        } else {
            self.pressed_buttons.remove(&code)
        };
        if !changed {
            return;
        }

        self.buttons.push((code, pressed));
        self.has_data = true;
    }

    /// Release every button still logically held by this virtual pointer.
    /// Call `flush` first so pending physical transitions are emitted in order.
    pub fn release_all(&mut self, vptr: &ZwlrVirtualPointerV1, time_ms: u32) {
        if self.pressed_buttons.is_empty() {
            return;
        }

        for code in std::mem::take(&mut self.pressed_buttons) {
            vptr.button(time_ms, code, ButtonState::Released);
        }
        vptr.frame();
    }

    pub fn accumulate_scroll_vert(&mut self, value: i32) {
        self.scroll_vert += value as f64;
        self.has_data = true;
    }

    pub fn accumulate_scroll_horiz(&mut self, value: i32) {
        self.scroll_horiz += value as f64;
        self.has_data = true;
    }

    pub fn accumulate_abs(&mut self, x: u32, y: u32) {
        self.abs = Some((x, y));
        self.has_data = true;
    }

    /// Send all accumulated events to the virtual pointer, then reset.
    ///
    /// Flush order (per wlr-virtual-pointer spec):
    /// 1. Relative motion
    /// 2. Absolute motion (requires extent)
    /// 3. Vertical scroll (axis 0)
    /// 4. Horizontal scroll (axis 1)
    /// 5. Buttons (replayed in order)
    /// 6. frame()
    pub fn flush(&mut self, vptr: &ZwlrVirtualPointerV1, time_ms: u32, extent: Option<(u32, u32)>) {
        if !self.has_data {
            return;
        }

        // 1. Relative motion
        if self.dx != 0.0 || self.dy != 0.0 {
            vptr.motion(time_ms, self.dx, self.dy);
        }

        // 2. Absolute motion
        if let (Some((x, y)), Some((ext_w, ext_h))) = (self.abs, extent) {
            vptr.motion_absolute(time_ms, x, y, ext_w, ext_h);
        }

        // 3. Vertical scroll
        if self.scroll_vert != 0.0 {
            vptr.axis(time_ms, Axis::VerticalScroll, self.scroll_vert);
        }

        // 4. Horizontal scroll
        if self.scroll_horiz != 0.0 {
            vptr.axis(time_ms, Axis::HorizontalScroll, self.scroll_horiz);
        }

        // 5. Buttons — replayed in order, not coalesced
        for &(code, pressed) in &self.buttons {
            let state = if pressed {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            };
            vptr.button(time_ms, code, state);
        }

        // 6. Frame
        vptr.frame();

        // 7. Reset
        self.dx = 0.0;
        self.dy = 0.0;
        self.buttons.clear();
        self.scroll_vert = 0.0;
        self.scroll_horiz = 0.0;
        self.abs = None;
        self.has_data = false;
    }
}

#[cfg(test)]
mod tests {
    use super::MouseFrame;

    const BTN_LEFT: u32 = 0x110;

    #[test]
    fn pointer_repeat_does_not_create_a_second_press() {
        let mut frame = MouseFrame::new();

        frame.accumulate_button(BTN_LEFT, 1);
        frame.accumulate_button(BTN_LEFT, 2);
        frame.accumulate_button(BTN_LEFT, 0);

        assert_eq!(frame.buttons, vec![(BTN_LEFT, true), (BTN_LEFT, false)]);
        assert!(frame.pressed_buttons.is_empty());
    }

    #[test]
    fn duplicate_transitions_are_discarded() {
        let mut frame = MouseFrame::new();

        frame.accumulate_button(BTN_LEFT, 0);
        frame.accumulate_button(BTN_LEFT, 1);
        frame.accumulate_button(BTN_LEFT, 1);
        frame.accumulate_button(BTN_LEFT, 0);
        frame.accumulate_button(BTN_LEFT, 0);

        assert_eq!(frame.buttons, vec![(BTN_LEFT, true), (BTN_LEFT, false)]);
        assert!(frame.pressed_buttons.is_empty());
    }
}
