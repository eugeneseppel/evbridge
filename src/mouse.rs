use wayland_client::protocol::wl_pointer::{Axis, ButtonState};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

/// Accumulated mouse events within a single frame (between SYN_REPORTs).
/// Flushed atomically to the Wayland virtual pointer.
#[derive(Debug)]
pub struct MouseFrame {
    dx: f64,
    dy: f64,
    buttons: Vec<(u32, bool)>,
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

    pub fn accumulate_button(&mut self, code: u32, pressed: bool) {
        self.buttons.push((code, pressed));
        self.has_data = true;
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
