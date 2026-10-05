mod classify;
mod keyboard;
mod mouse;
mod watcher;
mod wayland;

use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use calloop::generic::Generic;
use calloop::{EventLoop, Interest, Mode, PostAction};
use calloop_wayland_source::WaylandSource;
use clap::Parser;
use evdev::{AbsoluteAxisCode, EventType, RelativeAxisCode};
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::classify::{DeviceCaps, DeviceType};
use crate::mouse::MouseFrame;
use crate::watcher::{WatchEvent, Watcher};
use crate::wayland::AppState;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(name = "evbridge", version, about = "evdev to Wayland input bridge")]
struct Cli {
    #[arg(long)]
    wayland_display: Option<String>,
    /// Name of the target wl_seat. Required when the compositor advertises more than one seat.
    #[arg(long)]
    seat_name: Option<String>,
    /// Bridge an existing evdev device using automatic capability classification.
    /// May be specified more than once.
    #[arg(long)]
    device: Vec<PathBuf>,
    /// Bridge only pointer events from an existing evdev device, ignoring any
    /// keyboard-like capabilities it also advertises. May be specified more than once.
    #[arg(long)]
    mouse_device: Vec<PathBuf>,
    /// Exclusively grab bridged evdev devices so other host applications do not receive events.
    #[arg(long, default_value_t = false)]
    grab: bool,
    #[arg(long, default_value = "/dev/input")]
    input_dir: PathBuf,
    #[arg(long, default_value = "us")]
    xkb_layout: String,
    #[arg(long, default_value = "")]
    xkb_variant: String,
    #[arg(long, default_value = "")]
    xkb_options: String,
    #[arg(long, default_value = "info")]
    log_level: String,
    /// Filter input devices by phys field (e.g. "sunshine/hostname").
    /// Only devices whose /sys/class/input/eventN/device/phys contains this string are accepted.
    #[arg(long, env = "EVBRIDGE_PHYS_FILTER")]
    phys_filter: Option<String>,
    /// Create missing /dev/input nodes for newly appearing sysfs devices.
    /// This needs CAP_MKNOD and is disabled by default.
    #[arg(long, default_value_t = false)]
    create_missing_device_nodes: bool,
}

// ---------------------------------------------------------------------------
// FdWrapper — wraps a raw fd so calloop can poll it without owning it
// ---------------------------------------------------------------------------

/// SAFETY: The caller must ensure the underlying fd owner outlives the calloop
/// source registration. remove_device() must remove the calloop source BEFORE
/// dropping the evdev::Device that owns the fd.
struct FdWrapper(RawFd);

#[derive(Clone, Copy)]
enum DeviceRole {
    Auto,
    PointerOnly,
}

impl AsFd for FdWrapper {
    fn as_fd(&self) -> BorrowedFd<'_> {
        unsafe { BorrowedFd::borrow_raw(self.0) }
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        // Do not read RUST_LOG: service-manager or inherited environment must not
        // be able to override the explicitly requested logging policy.
        .with_env_filter(EnvFilter::new(&cli.log_level))
        .init();

    info!("evbridge starting");

    // 1. Connect to Wayland
    let (conn, event_queue, mut state) = wayland::connect(
        cli.wayland_display.as_deref(),
        cli.seat_name.as_deref(),
    )?;
    info!("connected to Wayland compositor");

    // 2. Generate XKB keymap
    let keymap_str = keyboard::generate_keymap(&cli.xkb_layout, &cli.xkb_variant, &cli.xkb_options);
    state.keymap_str = keymap_str;
    info!(layout = %cli.xkb_layout, "generated XKB keymap");

    // 3. Create calloop event loop
    let mut event_loop: EventLoop<'static, AppState> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    // 4. Insert Wayland source
    let wayland_source = WaylandSource::new(conn, event_queue);
    loop_handle.insert_source(wayland_source, |_, queue, state| {
        queue.dispatch_pending(state)
    })?;

    if !cli.device.is_empty() || !cli.mouse_device.is_empty() {
        for device in &cli.device {
            add_device(
                device,
                &mut state,
                &loop_handle,
                cli.grab,
                DeviceRole::Auto,
            )?;
        }
        for device in &cli.mouse_device {
            add_device(
                device,
                &mut state,
                &loop_handle,
                cli.grab,
                DeviceRole::PointerOnly,
            )?;
        }
        info!(
            device_count = cli.device.len() + cli.mouse_device.len(),
            "exact-device mode active; directory watching and mknod disabled"
        );
    } else {
        // Create watcher (snapshots sysfs baseline), scan existing devices.
        let watcher = Watcher::new(&cli.input_dir, cli.phys_filter.as_deref())?;
        let existing = watcher.scan_existing();
        for path in &existing {
            if let Err(e) = add_device(
                path,
                &mut state,
                &loop_handle,
                cli.grab,
                DeviceRole::Auto,
            ) {
                warn!(?path, err = %e, "failed to add existing device");
            }
        }

        // Insert inotify watcher as calloop source.
        let watcher_fd = watcher.as_fd().as_raw_fd();
        let watcher_cell = std::rc::Rc::new(std::cell::RefCell::new(watcher));

        let watcher_for_cb = watcher_cell.clone();
        let loop_handle_for_cb = loop_handle.clone();
        let grab_for_cb = cli.grab;

        loop_handle.insert_source(
            Generic::new(FdWrapper(watcher_fd), Interest::READ, Mode::Level),
            move |_, _, state| {
                let mut w = watcher_for_cb.borrow_mut();
                let events = match w.read_events() {
                    Ok(events) => events,
                    Err(e) => {
                        error!(err = %e, "failed to read inotify events");
                        return Ok(PostAction::Continue);
                    }
                };
                for event in events {
                    match event {
                        WatchEvent::DeviceAdded(path) => {
                            if let Err(e) =
                                add_device(
                                    &path,
                                    state,
                                    &loop_handle_for_cb,
                                    grab_for_cb,
                                    DeviceRole::Auto,
                                )
                            {
                                warn!(?path, err = %e, "failed to add device");
                            }
                        }
                        WatchEvent::DeviceRemoved(path) => {
                            remove_device(&path, state, &loop_handle_for_cb);
                        }
                    }
                }
                Ok(PostAction::Continue)
            },
        )?;

        // Device-node creation is privileged and opt-in. It is only useful when
        // uinput devices appear in sysfs but not in a bind-mounted /dev/input.
        if cli.create_missing_device_nodes {
            let watcher_for_timer = watcher_cell.clone();
            loop_handle.insert_source(
                calloop::timer::Timer::from_duration(std::time::Duration::from_millis(500)),
                move |_, _, _state| {
                    let w = watcher_for_timer.borrow();
                    w.ensure_device_nodes();
                    calloop::timer::TimeoutAction::ToDuration(std::time::Duration::from_millis(500))
                },
            )?;
        }
    }

    // 8. SIGINT/SIGTERM handling
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })?;

    // 9. Run event loop
    info!("entering event loop");
    while running.load(Ordering::SeqCst) {
        event_loop.dispatch(std::time::Duration::from_millis(100), &mut state)?;
    }

    // Graceful shutdown: destroy all virtual devices
    info!("evbridge shutting down");
    for (path, tracked) in state.devices.drain() {
        if let Some(vptr) = tracked.pointer {
            vptr.destroy();
        }
        if let Some(vkbd) = tracked.keyboard {
            vkbd.destroy();
        }
        info!(?path, "destroyed virtual device");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// create_udev_joystick_entry — write /run/udev/data/c{major}:{minor} so
// SDL/Steam can discover gamepads via libudev
// ---------------------------------------------------------------------------

fn create_udev_joystick_entry(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::MetadataExt;

    let meta = std::fs::metadata(path)?;
    let rdev = meta.rdev();
    let major = rustix::fs::major(rdev);
    let minor = rustix::fs::minor(rdev);

    let udev_dir = Path::new("/run/udev/data");
    std::fs::create_dir_all(udev_dir)?;

    let entry_path = udev_dir.join(format!("c{}:{}", major, minor));
    std::fs::write(
        &entry_path,
        "E:ID_INPUT=1\nE:ID_INPUT_JOYSTICK=1\nE:ID_BUS=usb\nG:seat\nV:1\n",
    )?;

    info!(path = %entry_path.display(), major, minor, "created udev joystick entry");
    Ok(())
}

// ---------------------------------------------------------------------------
// add_device
// ---------------------------------------------------------------------------

fn add_device(
    path: &Path,
    state: &mut AppState,
    loop_handle: &calloop::LoopHandle<'static, AppState>,
    grab: bool,
    role: DeviceRole,
) -> Result<(), Box<dyn std::error::Error>> {
    if state.devices.contains_key(path) {
        debug!(?path, "device already tracked, skipping");
        return Ok(());
    }

    let mut dev = evdev::Device::open(path)?;
    dev.set_nonblocking(true)?;

    let caps = DeviceCaps::from_device(&dev);
    let types = classify::classify(&caps);

    if types.is_empty() {
        debug!(?path, "unknown device type, skipping");
        return Ok(());
    }

    if matches!(role, DeviceRole::Auto) && types.contains(&DeviceType::Gamepad) {
        info!(
            ?path,
            "gamepad detected, creating udev db entry for SDL/Steam"
        );
        if let Err(e) = create_udev_joystick_entry(path) {
            warn!(?path, err = %e, "failed to create udev db entry");
        }
        return Ok(());
    }

    if grab {
        dev.grab().map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("failed to exclusively grab {}: {e}", path.display()),
            )
        })?;
        info!(?path, "exclusively grabbed input device");
    }

    let supports_mouse =
        types.contains(&DeviceType::Mouse) || types.contains(&DeviceType::AbsoluteMouse);
    let supports_keyboard = types.contains(&DeviceType::Keyboard);

    let (has_mouse, has_keyboard) = match role {
        DeviceRole::Auto => (supports_mouse, supports_keyboard),
        DeviceRole::PointerOnly if supports_mouse => (true, false),
        DeviceRole::PointerOnly => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} does not advertise pointer capabilities", path.display()),
            )
            .into());
        }
    };

    let pointer = if has_mouse {
        let vptr = state.wl.create_virtual_pointer(&state.qh);
        info!(?path, "created virtual pointer");
        Some(vptr)
    } else {
        None
    };

    let (vkbd, kbd_state) = if has_keyboard {
        let vk = state.wl.create_virtual_keyboard(&state.qh);
        keyboard::send_keymap(&vk, &state.keymap_str)?;
        let ks = keyboard::KeyboardState::new(&state.keymap_str);
        info!(?path, "created virtual keyboard with keymap");
        (Some(vk), Some(ks))
    } else {
        (None, None)
    };

    // Get absolute axis extent for absolute mice
    let extent = if types.contains(&DeviceType::AbsoluteMouse) {
        match dev.get_abs_state() {
            Ok(states) => {
                let x_info = states[AbsoluteAxisCode::ABS_X.0 as usize];
                let y_info = states[AbsoluteAxisCode::ABS_Y.0 as usize];
                let w = (x_info.maximum - x_info.minimum) as u32;
                let h = (y_info.maximum - y_info.minimum) as u32;
                if w > 0 && h > 0 {
                    info!(?path, w, h, "absolute mouse extent");
                    Some((w, h))
                } else {
                    warn!(?path, "absolute mouse has zero extent");
                    None
                }
            }
            Err(e) => {
                warn!(?path, err = %e, "failed to read abs state");
                None
            }
        }
    } else {
        None
    };

    // Register evdev fd in calloop
    let raw_fd = dev.as_raw_fd();
    let path_for_cb = path.to_path_buf();
    let token = loop_handle.insert_source(
        Generic::new(FdWrapper(raw_fd), Interest::READ, Mode::Level),
        move |_, _, state| {
            let should_remove = state
                .devices
                .get_mut(&path_for_cb)
                .is_some_and(process_events);
            if should_remove {
                // Remove dead device and destroy virtual objects
                if let Some(tracked) = state.devices.remove(&path_for_cb) {
                    if let Some(vptr) = tracked.pointer {
                        vptr.destroy();
                    }
                    if let Some(vkbd) = tracked.keyboard {
                        vkbd.destroy();
                    }
                    info!(path = ?path_for_cb, "removed dead device (ENODEV)");
                }
                return Ok(PostAction::Remove);
            }
            Ok(PostAction::Continue)
        },
    )?;

    state.devices.insert(
        path.to_path_buf(),
        wayland::TrackedDevice {
            evdev: dev,
            pointer,
            keyboard: vkbd,
            kbd_state,
            frame: MouseFrame::new(),
            extent,
            last_abs_x: 0,
            last_abs_y: 0,
            token,
        },
    );

    info!(?path, ?types, "device added");
    Ok(())
}

// ---------------------------------------------------------------------------
// remove_device
// ---------------------------------------------------------------------------

fn remove_device(
    path: &Path,
    state: &mut AppState,
    loop_handle: &calloop::LoopHandle<'static, AppState>,
) {
    if let Some(tracked) = state.devices.remove(path) {
        loop_handle.remove(tracked.token);
        if let Some(vptr) = tracked.pointer {
            vptr.destroy();
        }
        if let Some(vkbd) = tracked.keyboard {
            vkbd.destroy();
        }
        info!(?path, "device removed");
    } else {
        // Might be a gamepad (not tracked in state.devices) — clean up udev entry
        cleanup_udev_entry(path);
        debug!(?path, "remove_device called for unknown device");
    }
}

fn cleanup_udev_entry(path: &Path) {
    use std::os::unix::fs::MetadataExt;

    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return,
    };
    let rdev = meta.rdev();
    let major = rustix::fs::major(rdev);
    let minor = rustix::fs::minor(rdev);
    let entry_path = Path::new("/run/udev/data").join(format!("c{}:{}", major, minor));
    if entry_path.exists() {
        let _ = std::fs::remove_file(&entry_path);
        info!(path = %entry_path.display(), "removed udev entry");
    }
}

// ---------------------------------------------------------------------------
// process_events
// ---------------------------------------------------------------------------

/// Returns `true` if the device is dead and should be removed.
fn process_events(tracked: &mut wayland::TrackedDevice) -> bool {
    let events = match tracked.evdev.fetch_events() {
        Ok(events) => events,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return false,
        Err(e) => {
            warn!(err = %e, "device read error, marking for removal");
            return true;
        }
    };

    let mut abs_dirty = false;

    for ev in events {
        let time_ms = ev
            .timestamp()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as u32)
            .unwrap_or(0);

        match ev.event_type() {
            EventType::RELATIVE => {
                let code = RelativeAxisCode(ev.code());
                let value = ev.value();
                match code {
                    RelativeAxisCode::REL_X => tracked.frame.accumulate_rel_x(value),
                    RelativeAxisCode::REL_Y => tracked.frame.accumulate_rel_y(value),
                    RelativeAxisCode::REL_WHEEL => tracked.frame.accumulate_scroll_vert(value),
                    RelativeAxisCode::REL_HWHEEL => tracked.frame.accumulate_scroll_horiz(value),
                    _ => {}
                }
            }
            EventType::ABSOLUTE => {
                let code = AbsoluteAxisCode(ev.code());
                let value = ev.value();
                match code {
                    AbsoluteAxisCode::ABS_X => {
                        tracked.last_abs_x = value as u32;
                        abs_dirty = true;
                    }
                    AbsoluteAxisCode::ABS_Y => {
                        tracked.last_abs_y = value as u32;
                        abs_dirty = true;
                    }
                    _ => {}
                }
            }
            EventType::KEY => {
                let code = ev.code();
                let value = ev.value();
                let pressed = value != 0;

                // BTN range 0x110..=0x11f = mouse buttons
                if (0x110..=0x11f).contains(&code) {
                    if tracked.pointer.is_some() {
                        tracked.frame.accumulate_button(code as u32, pressed);
                    }
                } else if let (Some(vkbd), Some(ks)) = (&tracked.keyboard, &mut tracked.kbd_state) {
                    ks.forward_key(vkbd, time_ms, code as u32, pressed);
                }
            }
            EventType::SYNCHRONIZATION => {
                // Commit absolute position if either axis updated this frame
                if abs_dirty {
                    tracked
                        .frame
                        .accumulate_abs(tracked.last_abs_x, tracked.last_abs_y);
                    abs_dirty = false;
                }

                // Flush mouse frame
                if let Some(ref vptr) = tracked.pointer {
                    tracked.frame.flush(vptr, time_ms, tracked.extent);
                }
            }
            _ => {}
        }
    }
    false
}
