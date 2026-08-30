# Wayland backend design

Date: 2026-08-31
Status: draft, awaiting review

## Motivation

Proactive support for GUI apps that only speak Wayland, with no XWayland
fallback (X11 mode cannot run these at all today). No specific app is
blocked right now — this is future-proofing.

`gnome-terminal` and `nautilus` were checked as possible motivating
examples for this work, since both failed under the current X11/Xvfb
native runtime ("application did not map a window within 15.0s"). That
turned out to be unrelated to Wayland — see
[Related but separate](#related-but-separate-gnome-terminal--nautilus-session-requirements)
below. They don't motivate this design; the motivation stays "apps with
no X11 path at all."

## Goals / success criteria

Feature parity with the existing X11 native backend:

- frame capture
- mouse + keyboard input injection
- dynamic resize
- UTF-8 clipboard
- window (re-)selection and crash recovery (the two bugs fixed today:
  picking the right window among several, and not trusting a cached
  window handle that can go stale)

This is additive: existing X11/XWayland apps keep working through the
existing `X11Display`/`NativeSession` path, unchanged.

## Approach

**Chosen: a headless wlroots compositor (`sway`, `WLR_BACKEND=headless`)
plus `wlr-*` protocol clients** — the Wayland-side mirror of today's
"Xvfb + X11Display" shape.

Rejected:

- **xdg-desktop-portal + PipeWire** (the GNOME/KDE screen-share path). Needs
  a D-Bus session bus, PipeWire, and a portal daemon, and is designed
  around an interactive "allow this app to share your screen?" consent
  dialog. Doesn't fit a fully automated, headless container.
- **Per-app bespoke integration.** Not applicable without a concrete
  target app to integrate against.

sway is a pragmatic choice over writing a compositor from scratch
(e.g. with `smithay`): it already implements every protocol this needs
(`wlr-screencopy`, `wlr-virtual-pointer`, `virtual-keyboard`,
`wlr-output-management`) and ships a scriptable IPC (`swaymsg`) for
window/output queries, so none of that has to be built or maintained
in-house.

## Architecture

Mirrors the existing split:

| Today (X11)                | Wayland equivalent                                  |
|-----------------------------|------------------------------------------------------|
| `Xvfb` (headless X server)  | `WaylandCompositor`: spawns `sway` with `WLR_BACKEND=headless WLR_LIBINPUT_NO_DEVICES=1`, waits for its IPC socket, sets the headless output's resolution via `swaymsg` |
| `X11Display`                | `WaylandDisplay`: a `wayland-client` connection to that compositor |
| SHM capture (`shm_get_image`) | `wlr-screencopy-unstable-v1` (also SHM-backed)     |
| `xtest_fake_input`          | `wlr-virtual-pointer-unstable-v1` + `virtual-keyboard-unstable-v1` |
| RandR `set_crtc_config`     | `swaymsg output ... resolution WxH` (or `wlr-output-management-unstable-v1` directly) |
| `query_tree` + largest-window heuristic | `swaymsg -t get_tree` (JSON) + same largest-view heuristic |
| X11 selection ownership     | `wlr-data-control-unstable-v1` (background clipboard access without holding keyboard focus, closer to what X11 selection ownership lets us do today than the focus-gated `wl_data_device_manager`) |

`NativeSession` gains a second backend variant (`Wayland`, alongside
`X11`) behind the same public API it exposes today
(`capture`/`inject`/`resize`/`is_running`/etc.), so the wire protocol,
tile-diffing renderer, and Kitty-protocol terminal output are reused
untouched — only the capture/injection/resize layer is new.

## Error handling

Carries forward both of today's fixes into the Wayland path from the
start, rather than re-discovering them later:

- Re-resolve "the" window/view via `swaymsg -t get_tree` before every
  resize, instead of caching a view id — mirrors the `BadWindow`
  stale-handle fix.
- A single failed input injection is logged and dropped, not fatal —
  mirrors the agent.rs non-fatal-injection fix.

## New dependencies

- Rust: `wayland-client`, `wayland-protocols`, `wayland-protocols-wlr`.
- Container image: `sway` (pulls in wlroots) for any Wayland-target
  example/image.

## Testing

Mirror `native.rs`'s existing Xvfb-gated `#[ignore]` convention: spin up
the headless sway compositor (+ a synthetic Wayland surface, or a real
minimal Wayland-only app) in a test gated behind
`#[ignore = "requires headless sway"]`, and add a corresponding CI step
alongside the existing "Xvfb integration tests" one.

## Open questions

1. **Backend selection.** How does an image/app declare "run me under
   Wayland" vs the default X11 path — a new explicit `--runtime` value
   (mirroring today's `native|oci|oci-agent|firecrab`), or something
   else? Leaning toward explicit, matching the existing pattern; not
   auto-detected, since most apps that support Wayland also support X11
   and there's no reliable way to know which a given binary needs without
   trying it.
2. **Clipboard protocol availability.** Confirm `wlr-data-control-unstable-v1`
   is what we want (background access, no focus requirement) over
   `wl_data_device_manager` (focus-gated) before implementing.
3. **Multi-surface Wayland apps.** Rare, but possible — same
   largest-surface heuristic as the X11 fix; revisit if a real case
   surfaces.

## Related but separate: gnome-terminal / nautilus session requirements

Checked empirically this session — neither app's failure was about
Wayland:

- **nautilus** fails immediately with `Failed to initialize display
  server connection: Unsupported or missing session type 'tty'` unless
  `XDG_SESSION_TYPE` is set (e.g. to `x11`). Setting it gets past that
  check entirely.
- **gnome-terminal** is D-Bus-activated (it asks a `org.gnome.Terminal`
  D-Bus service to open a window) and does nothing observable at all
  without a running D-Bus session bus. Wrapping it in `dbus-run-session`
  got it through service activation
  (`org.gnome.Terminal`, `org.gtk.vfs.Daemon`, `xdg-desktop-portal`) far
  past where it stalled before.

This is a separate, much smaller fix — set `XDG_SESSION_TYPE` and run a
D-Bus session bus around the app command in the native runtime — that
would likely unblock many GNOME-family apps regardless of X11 or
Wayland. Worth doing independently of this design, not a prerequisite
or a blocker for it.
