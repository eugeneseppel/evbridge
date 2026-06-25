use std::collections::HashMap;
use std::path::PathBuf;

use calloop::RegistrationToken;
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, delegate_noop,
    protocol::{wl_registry, wl_seat},
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

use crate::keyboard::KeyboardState;
use crate::mouse::MouseFrame;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Holds Wayland globals obtained from wl_registry.
pub struct WaylandState {
    pub seat: Option<wl_seat::WlSeat>,
    pub vptr_mgr: Option<ZwlrVirtualPointerManagerV1>,
    pub vkbd_mgr: Option<ZwpVirtualKeyboardManagerV1>,
}

impl WaylandState {
    fn new() -> Self {
        Self {
            seat: None,
            vptr_mgr: None,
            vkbd_mgr: None,
        }
    }

    /// Returns `Err` listing any missing protocols, or `Ok(())` if all are bound.
    pub fn check_protocols(&self) -> Result<(), String> {
        let mut missing = Vec::new();
        if self.seat.is_none() {
            missing.push("wl_seat");
        }
        if self.vptr_mgr.is_none() {
            missing.push("zwlr_virtual_pointer_manager_v1");
        }
        if self.vkbd_mgr.is_none() {
            missing.push("zwp_virtual_keyboard_manager_v1");
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(format!("missing Wayland protocols: {}", missing.join(", ")))
        }
    }

    /// Create a virtual pointer via the bound manager.
    pub fn create_virtual_pointer(&self, qh: &QueueHandle<AppState>) -> ZwlrVirtualPointerV1 {
        let mgr = self.vptr_mgr.as_ref().expect("vptr_mgr not bound");
        let seat = self.seat.as_ref().expect("wl_seat not bound");
        mgr.create_virtual_pointer(Some(seat), qh, ())
    }

    /// Create a virtual keyboard via the bound manager.
    pub fn create_virtual_keyboard(&self, qh: &QueueHandle<AppState>) -> ZwpVirtualKeyboardV1 {
        let mgr = self.vkbd_mgr.as_ref().expect("vkbd_mgr not bound");
        let seat = self.seat.as_ref().expect("wl_seat not bound");
        mgr.create_virtual_keyboard(seat, qh, ())
    }
}

/// A tracked input device with its virtual Wayland counterparts.
pub struct TrackedDevice {
    pub evdev: evdev::Device,
    pub pointer: Option<ZwlrVirtualPointerV1>,
    pub keyboard: Option<ZwpVirtualKeyboardV1>,
    pub kbd_state: Option<KeyboardState>,
    pub frame: MouseFrame,
    pub extent: Option<(u32, u32)>,
    pub last_abs_x: u32,
    pub last_abs_y: u32,
    pub token: RegistrationToken,
}

/// Top-level state for the calloop event loop.
pub struct AppState {
    pub wl: WaylandState,
    pub devices: HashMap<PathBuf, TrackedDevice>,
    pub qh: QueueHandle<AppState>,
    pub keymap_str: String,
}

// ---------------------------------------------------------------------------
// Dispatch: wl_registry — bind globals on advertisement
// ---------------------------------------------------------------------------

impl Dispatch<wl_registry::WlRegistry, ()> for AppState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_seat" => {
                    let seat =
                        registry.bind::<wl_seat::WlSeat, _, Self>(name, version.min(9), qh, ());
                    state.wl.seat = Some(seat);
                }
                "zwlr_virtual_pointer_manager_v1" => {
                    let mgr = registry.bind::<ZwlrVirtualPointerManagerV1, _, Self>(
                        name,
                        version.min(1),
                        qh,
                        (),
                    );
                    state.wl.vptr_mgr = Some(mgr);
                }
                "zwp_virtual_keyboard_manager_v1" => {
                    let mgr = registry.bind::<ZwpVirtualKeyboardManagerV1, _, Self>(
                        name,
                        version.min(1),
                        qh,
                        (),
                    );
                    state.wl.vkbd_mgr = Some(mgr);
                }
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// No-op dispatchers for interfaces we bind but never handle events from.
// ---------------------------------------------------------------------------

delegate_noop!(AppState: ignore wl_seat::WlSeat);
delegate_noop!(AppState: ignore ZwlrVirtualPointerManagerV1);
delegate_noop!(AppState: ignore ZwlrVirtualPointerV1);
delegate_noop!(AppState: ignore ZwpVirtualKeyboardManagerV1);
delegate_noop!(AppState: ignore ZwpVirtualKeyboardV1);

// ---------------------------------------------------------------------------
// connect() — establish Wayland connection and bind all required globals
// ---------------------------------------------------------------------------

/// Connect to a Wayland compositor, perform a roundtrip to bind globals,
/// and verify that all required protocols are available.
///
/// If `display` is `Some`, `WAYLAND_DISPLAY` is set before connecting.
/// Returns the connection, event queue, and fully-initialised AppState.
pub fn connect(
    display: Option<&str>,
) -> Result<(Connection, EventQueue<AppState>, AppState), Box<dyn std::error::Error>> {
    if let Some(name) = display {
        // SAFETY: called before any threads are spawned (single-threaded init).
        unsafe { std::env::set_var("WAYLAND_DISPLAY", name) };
    }

    let conn = Connection::connect_to_env()?;
    let display_obj = conn.display();
    let mut event_queue: EventQueue<AppState> = conn.new_event_queue();
    let qh = event_queue.handle();

    // Request the registry — this triggers Global events on roundtrip.
    display_obj.get_registry(&qh, ());

    // Temporary state to receive globals.
    let mut state = AppState {
        wl: WaylandState::new(),
        devices: HashMap::new(),
        qh: qh.clone(),
        keymap_str: String::new(), // set by caller after connect
    };

    // Roundtrip to receive all global advertisements.
    event_queue.roundtrip(&mut state)?;

    // Verify all required protocols were advertised.
    state
        .wl
        .check_protocols()
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

    Ok((conn, event_queue, state))
}
