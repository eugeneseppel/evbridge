# evbridge

Bridge Linux **evdev** input devices into a **Wayland** compositor.

`evbridge` watches `/dev/input`, and for every input device it finds it creates a
matching **wlroots virtual pointer / virtual keyboard** in a running Wayland
compositor and forwards events to it. It's built for **headless or nested Wayland**
sessions — game-streaming containers, remote desktops, CI — where the compositor
doesn't read `/dev/input` itself but you still want real (or emulated) input devices
to drive it.

## What it does

- **Hotplug-aware** — watches `/dev/input` via inotify; creates and tears down virtual
  devices as hardware appears and disappears.
- **Classifies** mice (relative), absolute mice/touch, keyboards, and gamepads.
- **Pointer** — relative + absolute motion, buttons, and vertical/horizontal scroll via
  `wlr_virtual_pointer`.
- **Keyboard** — builds an XKB keymap (configurable layout/variant/options) and forwards
  keys via `virtual_keyboard`.
- **Gamepads** — writes a `/run/udev/data` entry so SDL/Steam discover the pad via libudev.
- **Robust** — drops dead devices, destroys all virtual devices on exit, and periodically
  `mknod`s device nodes that exist in `/sys/class/input` but not in a bind-mounted
  `/dev/input` (e.g. uinput devices created inside a container).

## Usage

```
evbridge [--wayland-display WAYLAND-1] [--input-dir /dev/input]
         [--seat-name player2] [--device PATH]... [--mouse-device PATH]... [--grab]
         [--xkb-layout us] [--xkb-variant ""] [--xkb-options ""]
         [--phys-filter SUBSTR] [--create-missing-device-nodes]
         [--log-level info]
```

- `--device PATH` — bridge an existing evdev device using automatic capability
  classification. May be repeated. Supplying any exact device bypasses directory watching
  and all device-node creation behavior.
- `--mouse-device PATH` — bridge only pointer motion, wheel, and mouse buttons from a
  device, ignoring keyboard-like capabilities it may also advertise. May be repeated.
- `--grab` — take an exclusive evdev grab while bridging. Other host applications stop
  receiving events from the selected device; the grab is released when evbridge exits.
- `--seat-name NAME` — select the named Wayland seat. If the compositor advertises
  multiple seats, this option is required.
- `--phys-filter SUBSTR` (or `EVBRIDGE_PHYS_FILTER`) — only bridge devices whose evdev
  `phys` field contains `SUBSTR`. Handy to bridge a specific virtual input source while
  ignoring the host's own devices.
- `--create-missing-device-nodes` — opt in to the privileged `mknod` compatibility path.
  It is disabled by default and should not be used for an existing host device.
- `--log-level LEVEL` — set the log filter explicitly. `RUST_LOG` is ignored, and input
  event values (keys, modifiers, pointer coordinates, scrolling, and buttons) are never
  written to logs at any level.

For a single keyboard routed to a nested Sway seat, use the supplied system service.
On hosts that restrict unprivileged user namespaces, raw `unshare` cannot create the
required UID mapping. The system manager creates the private network namespace, then
executes evbridge as the ordinary `user` account with no capabilities and `no_new_privs`.
The service joins the existing `input` group only for its own process, while its cgroup
device policy allows only the stable, read-only player2 keyboard and Logitech G309 mouse
nodes. The G309 is configured as mouse-only because its event interface also advertises
keyboard capabilities. The login user is not added to the broad `input` group. The service
permits only Unix sockets, denies IP traffic, and hides the user's home and runtime
directories. It bind
mounts only `/run/user/1000/wayland-1` back into the sandbox so the nested compositor
remains accessible without exposing the user D-Bus or systemd sockets.

The target compositor must expose the `wlr_virtual_pointer` and `virtual_keyboard`
protocols (wlroots-based compositors such as labwc or sway).

## Build & run

```bash
cargo build --release          # → target/release/evbridge
WAYLAND_DISPLAY=wayland-1 ./target/release/evbridge --phys-filter my-source
```

To build without installing Rust on the host:

```bash
docker build --file Dockerfile.build --target export \
  --output type=local,dest=dist .
```

To install and start the hardened system service after building:

```bash
sudo install -Dm755 dist/evbridge /usr/local/libexec/evbridge
sudo install -Dm644 contrib/evbridge-player2.service \
  /etc/systemd/system/evbridge-player2.service
sudo systemctl daemon-reload
sudo systemctl start evbridge-player2.service
sudo systemctl status evbridge-player2.service
```

The two `sudo install` commands copy reviewed files into root-owned locations. The service
itself runs the binary as `User=user`; untrusted application code is never executed as root.

Build/runtime needs `libwayland-client` and `libxkbcommon`. Prebuilt Linux binaries are
attached to each [release](https://github.com/atassis/evbridge/releases).

## License

Dual-licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE), at your option.
