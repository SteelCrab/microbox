# Changelog

## Unreleased

### Added

- Native Xvfb application lifecycle and X11 capture.
- Kitty Graphics full-frame and dirty-tile rendering.
- XDamage and MIT-SHM optimized capture with fallbacks.
- Keyboard, mouse, resize, and signal handling.
- Runtime diagnostics, FPS control, render statistics, smoke tests, and fuzz harness.
- Docker-backed OCI sessions with automatic image-reference detection, private
  X11 socket sharing, missing-image pulls, and deterministic container cleanup.
- Alpine `xeyes` OCI example for end-to-end smoke testing.

### Known limitations

- Native mode is not a security sandbox.
- One foreground application and the first mapped top-level window are supported.
- The X11 framebuffer remains fixed at 640×360 during a session.
- Firecrab, Wayland, persistent sessions, clipboard, and audio are not part of v0.1.
