# micro-gui

**GUI applications, without the desktop.**

micro-gui is an experimental runtime for displaying and controlling a single
Linux GUI application directly inside a terminal. It targets terminals that
implement the Kitty Graphics Protocol and does not require a desktop
environment, VNC, or RDP.

> Status: pre-alpha. Native Xvfb launch, X11 capture, Kitty Graphics output,
> keyboard/mouse forwarding, and terminal resize remapping are implemented.

## Why micro-gui?

Traditional remote GUI setups expose an entire desktop. micro-gui instead owns
the lifecycle of one application:

```text
GUI application → private X11 display → frame capture → micro-gui → terminal
terminal input  → micro-gui → coordinate mapping → X11 input
```

The project is independent from Firecrab. A future Firecrab backend will run the
same GUI session inside a disposable MicroVM, while the native backend remains
available for development and low-overhead use.

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

`run` creates a private 640×360 Xvfb display, expands the first mapped window,
captures the X11 root image at up to 30 FPS, and renders it in the terminal.
The terminal enters an alternate screen with mouse tracking; keyboard and mouse
events are forwarded through XTEST. Press `Ctrl-C` to stop the application and
its Xvfb process group.

XDamage skips unchanged captures, MIT-SHM is used when available, and small
changes are sent as 64-pixel tile overlays. The frame rate and render counters
can be inspected with:

```sh
micro-gui run xeyes --fps 30 --stats
```

The future Firecrab form remains reserved:

```sh
micro-gui run firefox --runtime firecrab
```

## v0.1 scope

- One foreground application per process
- Linux and an X11/Xvfb display backend
- Kitty Graphics full-frame and dirty-tile output
- Keyboard, mouse, and terminal-resize forwarding (fixed-size GUI framebuffer)
- Deterministic cleanup when the application or client exits

OCI images, background sessions (`ps`/`stop`), Wayland, and Firecrab are
post-v0.1 work.

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
