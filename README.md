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
         [--xkb-layout us] [--xkb-variant ""] [--xkb-options ""]
         [--phys-filter SUBSTR] [--log-level info]
```

- `--phys-filter SUBSTR` (or `EVBRIDGE_PHYS_FILTER`) — only bridge devices whose evdev
  `phys` field contains `SUBSTR`. Handy to bridge a specific virtual input source while
  ignoring the host's own devices.

The target compositor must expose the `wlr_virtual_pointer` and `virtual_keyboard`
protocols (wlroots-based compositors such as labwc or sway).

## Build & run

```bash
cargo build --release          # → target/release/evbridge
WAYLAND_DISPLAY=wayland-1 ./target/release/evbridge --phys-filter my-source
```

Build/runtime needs `libwayland-client` and `libxkbcommon`. Prebuilt Linux binaries are
attached to each [release](https://github.com/atassis/evbridge/releases).

## License

Dual-licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE), at your option.
