# Installation

## Build from source

Required components:

- Rust 1.85 or newer
- Linux
- `Xvfb`
- an X11 application such as `xeyes`
- a terminal implementing Kitty Graphics

```sh
./scripts/check-deps.sh
cargo build --release
cargo install --path .
micro-gui doctor
micro-gui demo
micro-gui run xeyes
```

The native backend is not a sandbox. It gives the application a private X11
display and process group, but shares the host filesystem, network, and kernel.

## Validation

```sh
cargo test
./scripts/smoke-test.sh
cargo clippy --all-targets -- -D warnings
```

An installed Mousepad can be tested separately because its startup depends on
the host D-Bus desktop session:

```sh
MICRO_GUI_GTK_SMOKE=1 \
  cargo test runtime::native::tests::smoke_tests_mousepad -- --ignored
```

The fuzz harness is optional:

```sh
cargo install cargo-fuzz
cargo fuzz run frame_and_input
```

## Terminal notes

`micro-gui doctor` uses terminal environment variables as a conservative hint.
If it reports `UNKNOWN`, run `micro-gui demo` for a visual protocol check. A
session always restores raw mode, mouse reporting, the cursor, and the alternate
screen on ordinary errors and handled termination signals. `SIGKILL` cannot be
intercepted by any process and therefore cannot run cleanup code.
