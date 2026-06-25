use evdev::{AbsoluteAxisCode, Device, EventType, KeyCode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Mouse,
    AbsoluteMouse,
    Keyboard,
    Gamepad,
}

/// Extracted capabilities for classification (testable without real devices).
pub struct DeviceCaps {
    pub has_ev_rel: bool,
    pub has_ev_abs: bool,
    pub has_ev_key: bool,
    pub has_btn_left: bool,
    pub has_key_a: bool,
    pub has_btn_gamepad: bool,
    pub has_abs_x: bool,
}

impl DeviceCaps {
    pub fn from_device(dev: &Device) -> Self {
        let events = dev.supported_events();
        let keys = dev.supported_keys();
        let abs = dev.supported_absolute_axes();
        Self {
            has_ev_rel: events.contains(EventType::RELATIVE),
            has_ev_abs: events.contains(EventType::ABSOLUTE),
            has_ev_key: events.contains(EventType::KEY),
            has_btn_left: keys.as_ref().is_some_and(|k| k.contains(KeyCode::BTN_LEFT)),
            has_key_a: keys.as_ref().is_some_and(|k| k.contains(KeyCode::KEY_A)),
            has_btn_gamepad: keys
                .as_ref()
                .is_some_and(|k| k.contains(KeyCode::BTN_SOUTH)),
            has_abs_x: abs
                .as_ref()
                .is_some_and(|a| a.contains(AbsoluteAxisCode::ABS_X)),
        }
    }
}

/// Classify a device by its capabilities. Returns all matching types
/// (a combo device may be both Mouse and Keyboard).
pub fn classify(caps: &DeviceCaps) -> Vec<DeviceType> {
    let mut types = Vec::new();

    // Gamepad: EV_ABS + BTN_GAMEPAD/BTN_SOUTH (check before abs mouse)
    if caps.has_ev_abs && caps.has_btn_gamepad {
        types.push(DeviceType::Gamepad);
        return types; // gamepads don't also register as mouse/keyboard
    }

    // Relative mouse: EV_REL + BTN_LEFT
    if caps.has_ev_rel && caps.has_btn_left {
        types.push(DeviceType::Mouse);
    }

    // Absolute mouse: EV_ABS + ABS_X + BTN_LEFT (and not gamepad)
    if caps.has_ev_abs && caps.has_abs_x && caps.has_btn_left {
        types.push(DeviceType::AbsoluteMouse);
    }

    // Keyboard: EV_KEY + KEY_A
    if caps.has_ev_key && caps.has_key_a {
        types.push(DeviceType::Keyboard);
    }

    types
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mouse_rel_with_btn_left() {
        let caps = DeviceCaps {
            has_ev_rel: true,
            has_ev_abs: false,
            has_ev_key: true,
            has_btn_left: true,
            has_key_a: false,
            has_btn_gamepad: false,
            has_abs_x: false,
        };
        let types = classify(&caps);
        assert!(types.contains(&DeviceType::Mouse));
        assert!(!types.contains(&DeviceType::Keyboard));
    }

    #[test]
    fn test_keyboard_with_key_a() {
        let caps = DeviceCaps {
            has_ev_rel: false,
            has_ev_abs: false,
            has_ev_key: true,
            has_btn_left: false,
            has_key_a: true,
            has_btn_gamepad: false,
            has_abs_x: false,
        };
        let types = classify(&caps);
        assert!(types.contains(&DeviceType::Keyboard));
        assert!(!types.contains(&DeviceType::Mouse));
    }

    #[test]
    fn test_gamepad_ignored() {
        let caps = DeviceCaps {
            has_ev_rel: false,
            has_ev_abs: true,
            has_ev_key: true,
            has_btn_left: false,
            has_key_a: false,
            has_btn_gamepad: true,
            has_abs_x: true,
        };
        let types = classify(&caps);
        assert!(types.contains(&DeviceType::Gamepad));
        assert!(!types.contains(&DeviceType::Mouse));
    }

    #[test]
    fn test_absolute_mouse() {
        let caps = DeviceCaps {
            has_ev_rel: false,
            has_ev_abs: true,
            has_ev_key: true,
            has_btn_left: true,
            has_key_a: false,
            has_btn_gamepad: false,
            has_abs_x: true,
        };
        let types = classify(&caps);
        assert!(types.contains(&DeviceType::AbsoluteMouse));
    }

    #[test]
    fn test_combo_device_mouse_and_keyboard() {
        let caps = DeviceCaps {
            has_ev_rel: true,
            has_ev_abs: false,
            has_ev_key: true,
            has_btn_left: true,
            has_key_a: true,
            has_btn_gamepad: false,
            has_abs_x: false,
        };
        let types = classify(&caps);
        assert!(types.contains(&DeviceType::Mouse));
        assert!(types.contains(&DeviceType::Keyboard));
    }

    #[test]
    fn test_unknown_device() {
        let caps = DeviceCaps {
            has_ev_rel: false,
            has_ev_abs: false,
            has_ev_key: false,
            has_btn_left: false,
            has_key_a: false,
            has_btn_gamepad: false,
            has_abs_x: false,
        };
        let types = classify(&caps);
        assert!(types.is_empty());
    }
}
