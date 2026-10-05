# Prioritized follow-ups

## Reliability

1. [ ] Handle input-device disconnects and reconnects. Exact-device mode currently
   leaves the process running but unable to forward input after a device disappears.
2. [ ] Tie the service lifecycle to the nested Sway instance. Avoid starting at
   general multi-user boot and hitting systemd's restart limit before the Wayland
   socket exists.

## Verification and maintenance

3. [ ] Add regression tests for:
   - repeated `--device` arguments;
   - `--mouse-device` ignoring keyboard-like capabilities;
   - grab failures being fatal;
   - named-seat selection;
   - fail-closed behavior when multiple seats exist;
   - the physical-device filter during initial enumeration.
4. [ ] Run formatting, Clippy, tests, and `cargo audit` in the Docker build
   environment. Keep Rust tooling off the host.
5. [x] Commit the working keyboard-and-mouse bridge, hardened service, and Docker
   build files on a dedicated branch.
6. [ ] Make deployment reproducible: build an identifiable artifact, install it
   atomically, validate the service unit, and document rollback.
7. [ ] Replace machine-specific values such as the user ID, Wayland display, and
   input-device paths with a generated unit or environment file.

## Input feature polish

8. [ ] Add support for high-resolution wheel events and additional mouse buttons.
9. [ ] Review keyboard repeat and layout behavior in the nested Sway session.
