# micro-gui

**GUI applications, without the desktop.**

micro-gui is an experimental runtime for displaying and controlling a single
Linux GUI application directly inside a terminal. It targets terminals that
implement the Kitty Graphics Protocol and does not require a desktop
environment, VNC, or RDP.

> Status: pre-alpha. Native and Docker/OCI launch, X11 capture, Kitty Graphics
> output, keyboard/mouse forwarding, and terminal resize remapping are
> implemented.

## Why micro-gui?

Traditional remote GUI setups expose an entire desktop. micro-gui instead owns
the lifecycle of one application:

```text
GUI application → private X11 display → frame capture → micro-gui → terminal
terminal input  → micro-gui → coordinate mapping → X11 input
```

The project is independent from Firecrab. The native and OCI backends are usable
without it. Firecrab currently lacks the bidirectional guest frame/input channel
needed for the planned MicroVM backend; micro-gui refuses that mode instead of
claiming isolation it cannot provide.

## Try the rendering milestone

Requirements:

- Rust 1.85 or newer
- Xvfb and the X11 application to run
- Kitty, Ghostty, WezTerm, or another terminal implementing Kitty Graphics

```sh
cargo run -- doctor
cargo run -- demo
cargo run -- run xeyes
cargo test
```

`demo` generates an RGB checkerboard and transmits it directly with the Kitty
Graphics Protocol. It verifies the first part of the rendering pipeline without
starting an X server.

The current native interface is:

```sh
micro-gui run xeyes
micro-gui run firefox
micro-gui run my-app -- --application-argument
```

An OCI reference containing a registry/repository separator is detected
automatically. `--runtime oci` (or the `docker` alias) is available for short
local image names:

```sh
docker build -t micro-gui/xeyes examples/oci-xeyes
micro-gui run micro-gui/xeyes
micro-gui run local-image --runtime oci -- --application-argument
```

The OCI backend pulls a missing image, starts a uniquely named disposable
container, shares only the private X11 Unix socket, enables
`no-new-privileges`, and force-removes that exact container on exit. Docker is
the engine for this initial OCI implementation.

Each foreground run publishes a user-private session record. From another
terminal, sessions can be inspected and stopped without guessing process IDs:

```sh
micro-gui ps
micro-gui stop gui-12345
```

`stop` sends `SIGTERM` only after matching both the recorded PID and its Linux
process start time, preventing a stale record from targeting a reused PID.
Records live below `$XDG_RUNTIME_DIR/micro-gui/sessions` (with a user-specific
temporary fallback), are mode `0600`, and disappear on normal or signal-driven
exit. This is cross-terminal control for foreground sessions; detachable
rendering and re-attachment are not implemented.

`run` creates a private 640×360 Xvfb display, expands the first mapped window,
captures the X11 root image at up to 30 FPS, and renders it in the terminal.
The terminal enters an alternate screen with mouse tracking; keyboard and mouse
events are forwarded through XTEST. Press `Ctrl-C` to stop the application and
its Xvfb process group.

Bracketed terminal paste is forwarded as one bounded UTF-8 X11 clipboard
transfer followed by the application's normal paste shortcut. Characters that
are not present in the X server keymap use the same path, so composed/IME text
can reach GTK applications without reducing it to ASCII. Clipboard payloads are
limited to 1 MiB. Automatic GUI-to-terminal clipboard export is intentionally
not enabled.

XDamage skips unchanged captures, MIT-SHM is used when available, and small
changes are sent as 64-pixel tile overlays. The frame rate and render counters
can be inspected with:

```sh
micro-gui run xeyes --fps 30 --stats
```

The Firecrab form connects to the authenticated micro-gui guest agent through a
Firecrab TCP port forward:

```sh
micro-gui run firefox --runtime firecrab
```

See [the Firecrab transport guide](docs/firecrab.md) for the agent image,
authentication, port-forward configuration, and current control-plane boundary.

## v0.1 scope

- One foreground application per process
- Linux and an X11/Xvfb display backend
- Kitty Graphics full-frame and dirty-tile output
- Keyboard, mouse, and terminal-resize forwarding (fixed-size GUI framebuffer)
- Deterministic cleanup when the application or client exits

Detachable sessions, Wayland, and Firecrab transport work remain post-v0.1.

See [the v0.1 architecture](docs/architecture-v0.1.md) and
[the implementation roadmap](docs/roadmap.md) for the concrete design and
acceptance criteria.

Build and system requirements are covered by [the installation guide](docs/install.md).

## Project principles

- Application runtime, not a desktop environment
- Explicit lifecycle ownership
- Runtime backends remain separate from terminal rendering
- No false isolation claims: native and MicroVM modes have distinct security
  boundaries
