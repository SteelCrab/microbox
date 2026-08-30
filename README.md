# micro-gui

**GUI applications, without the desktop.**

micro-gui is an experimental runtime for displaying and controlling a single
Linux GUI application directly inside a terminal. It targets terminals that
implement the Kitty Graphics Protocol and does not require a desktop
environment, VNC, or RDP.

> Status: pre-alpha. Terminal frame transmission works as a standalone demo;
> GUI capture and input injection are the next implementation milestone.

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
- Kitty, Ghostty, WezTerm, or another terminal implementing Kitty Graphics

```sh
cargo run -- doctor
cargo run -- demo
cargo test
```

`demo` generates an RGB checkerboard and transmits it directly with the Kitty
Graphics Protocol. It verifies the first part of the rendering pipeline without
starting an X server.

The intended product interface is:

```sh
micro-gui run xeyes
micro-gui run firefox
micro-gui run firefox --runtime firecrab
```

`run` is currently reserved and reports that the display backend is not yet
connected; it does not silently start an application on the host display.

## v0.1 scope

- One foreground application per process
- Linux and an X11/Xvfb display backend
- Kitty Graphics full-frame output, followed by dirty-tile updates
- Keyboard, mouse, and terminal-resize forwarding
- Deterministic cleanup when the application or client exits

OCI images, background sessions (`ps`/`stop`), Wayland, and Firecrab are
post-v0.1 work.

See [the v0.1 architecture](docs/architecture-v0.1.md) and
[the implementation roadmap](docs/roadmap.md) for the concrete design and
acceptance criteria.

## Project principles

- Application runtime, not a desktop environment
- Explicit lifecycle ownership
- Runtime backends remain separate from terminal rendering
- No false isolation claims: native and MicroVM modes have distinct security
  boundaries
