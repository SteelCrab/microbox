# Installation

## Build from source

Required components:

- Rust 1.85 or newer
- Kitty terminal (recommended and used as the primary validation target), or a
  terminal implementing the Kitty Graphics Protocol

Linux Native additionally requires `Xvfb` and an X11 application such as
`xeyes`. macOS requires Docker Desktop for local OCI execution; XQuartz is not
used.

```sh
./scripts/check-deps.sh
cargo build --release
cargo install --path .
microbox doctor
microbox demo
microbox run xeyes
```

### Linux

```sh
sudo apt-get install xvfb x11-apps x11-utils
microbox run xeyes
```

### macOS

Install Rust and Docker Desktop, then build an agent-enabled Linux GUI image:

```sh
brew install rust
docker build -f examples/firecrab-xeyes/Dockerfile \
  -t microbox/xeyes-agent .
cargo install --path .
microbox run microbox/xeyes-agent --runtime oci
```

For a platform-independent transport smoke test on Linux, run the same image
with `--runtime oci-agent`.

Both Apple Silicon (`aarch64-apple-darwin`) and Intel
(`x86_64-apple-darwin`) hosts are built in CI. `microbox run xeyes` without an
OCI or Firecrab runtime intentionally fails on macOS because a Mach-O host
cannot execute a Linux GUI binary. Use the OCI command above or connect to a
Firecrab guest.

The Linux native backend is not a sandbox. It gives the application a private X11
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
MICROBOX_GTK_SMOKE=1 \
  cargo test runtime::native::tests::smoke_tests_mousepad -- --ignored
```

The fuzz harness is optional:

```sh
cargo install cargo-fuzz
cargo fuzz run frame_and_input
```

## Terminal notes

`microbox doctor` uses terminal environment variables as a conservative hint.
If it reports `UNKNOWN`, run `microbox demo` for a visual protocol check. A
session always restores raw mode, mouse reporting, the cursor, and the alternate
screen on ordinary errors and handled termination signals. `SIGKILL` cannot be
intercepted by any process and therefore cannot run cleanup code.
