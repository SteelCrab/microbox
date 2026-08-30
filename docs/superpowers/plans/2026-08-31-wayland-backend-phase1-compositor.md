# Wayland Backend Phase 1: Headless Compositor Lifecycle — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `WaylandCompositor` type that spawns and controls a headless `sway` instance the same way `Xvfb` spawns and controls a headless X server today — this is the foundation the capture/input/clipboard work (later phases) builds on.

**Architecture:** `sway` (a wlroots compositor) run with `WLR_BACKENDS=headless`, controlled over its own IPC socket (a small, documented binary protocol — this plan implements it directly, no `swaymsg` subprocess calls). Mirrors the existing `Xvfb` struct in `src/runtime/native.rs` in shape and lifecycle: `start()` spawns the process and blocks until ready, `resize()` changes the output resolution, `Drop` tears everything down.

**Tech Stack:** Rust (existing crate conventions: hand-rolled protocol clients over `std::os::unix::net::UnixStream`, no async runtime). New dependency: `serde` + `serde_json`, for parsing sway's JSON-over-IPC replies (`wl_client`/`wlr-protocols` crates are NOT needed yet — this phase only talks to sway's control socket, not the Wayland display socket itself).

**Spec:** `docs/superpowers/specs/2026-08-31-wayland-backend-design.md`

## Global Constraints

- This phase is additive only: nothing in `src/display/x11.rs`, `src/runtime/native.rs`'s existing `Xvfb`/`NativeSession`/`X11Display` usage, or the wire protocol changes behavior. Existing tests must keep passing unmodified.
- Every new integration test that spawns a real `sway` process must be `#[ignore = "requires sway"]`, matching the existing `#[ignore = "requires Xvfb"]` / `#[ignore = "requires Xvfb and xeyes"]` convention in `src/runtime/native.rs`.
- Verified facts this plan depends on (established hands-on this session, not from documentation alone — see "Verification notes" at the end for how):
  - Alpine 3.24 ships `sway` 1.12-rc3 as `apk add sway`.
  - Running `sway` under Docker requires `--cap-add SYS_NICE` (the binary carries a `cap_sys_nice=ep` file capability; without this capability added to the container, `execve` itself fails with `EPERM`, not a runtime error).
  - `WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 sway` auto-creates one output named `HEADLESS-1` at a default 1280x720 — no manual `create_output` step needed on this version.
  - sway's IPC socket is a file matching `sway-ipc.<uid>.<pid>.sock` inside `$XDG_RUNTIME_DIR`; it is not predictable by name in advance and must be discovered by scanning the directory.
  - `swaymsg`/direct IPC needs `XDG_RUNTIME_DIR` set explicitly when connecting from outside sway's own process tree.
  - A minimal sway config of `xwayland disable\nbar {\n  swaybar_command /bin/true\n}\n` gives a clean startup with no functional errors (only benign, expected `drmGetDevices2 failed` / `swaybg` warnings that don't affect anything we use).
  - The sway IPC wire format (`man 7 sway-ipc`): 6-byte magic `i3-ipc`, then a 4-byte length and a 4-byte message-type field, both in **native** byte order, then that many payload bytes (JSON for replies). `RUN_COMMAND` is message type 0; `GET_OUTPUTS` is type 3. A `RUN_COMMAND` reply is a JSON array of `{"success": bool, ...}` objects, one per semicolon-separated command.

## Task 1: `WaylandCompositor` — spawn, discover its sockets, resize, tear down

**Files:**
- Modify: `Cargo.toml` — add `serde` and `serde_json` dependencies
- Modify: `src/runtime/native.rs:296-333` — widen `ManagedChild` and its methods from private to `pub(super)` so a sibling module can reuse them
- Create: `src/runtime/wayland.rs` — `WaylandCompositor`, `WaylandError`, the sway-IPC helper
- Modify: `src/runtime/mod.rs` — declare the new module and export its public types
- Modify: `.github/workflows/ci.yml` — install `sway` in the `linux` job so the new ignored test can run there

**Interfaces:**
- Produces: `pub struct WaylandCompositor` with:
  - `pub fn start(width: u16, height: u16) -> Result<Self, WaylandError>`
  - `pub fn resize(&mut self, width: u16, height: u16) -> Result<(), WaylandError>`
  - `pub fn wayland_socket_name(&self) -> &str` (e.g. `"wayland-1"` — later phases pass this as `WAYLAND_DISPLAY` when connecting a Wayland client)
  - `pub fn runtime_dir(&self) -> &std::path::Path` (later phases need this for `XDG_RUNTIME_DIR` when spawning the target app and when connecting their own Wayland client)
  - `impl Drop` that kills the sway process and removes the scratch directory
- Produces: `pub enum WaylandError` (`Debug + Display + std::error::Error`)
- Consumes (from `src/runtime/native.rs`, after this task widens their visibility): `pub(super) struct ManagedChild`, `pub(super) fn ManagedChild::spawn(&mut Command) -> std::io::Result<Self>`, `pub(super) fn ManagedChild::terminate(&mut self)`

---

- [ ] **Step 1: Add the JSON dependency**

Edit `Cargo.toml`, in the `[dependencies]` section, add these two lines (alphabetically, next to the existing entries):

```toml
serde = { version = "1.0.228", features = ["derive"] }
serde_json = "1.0.145"
```

- [ ] **Step 2: Fetch and build to confirm the dependency resolves**

Run: `cargo build 2>&1 | tail -20`
Expected: `Finished` with no errors (this just proves the crates resolve and compile; nothing uses them yet).

- [ ] **Step 3: Commit the dependency bump on its own**

```bash
git add Cargo.toml Cargo.lock
git commit -m "add: serde/serde_json for parsing sway IPC replies"
```

- [ ] **Step 4: Widen `ManagedChild`'s visibility**

In `src/runtime/native.rs`, change:

```rust
struct ManagedChild {
    child: Child,
    status: Option<ExitStatus>,
}

impl ManagedChild {
    fn spawn(command: &mut Command) -> std::io::Result<Self> {
```

to:

```rust
pub(super) struct ManagedChild {
    child: Child,
    status: Option<ExitStatus>,
}

impl ManagedChild {
    pub(super) fn spawn(command: &mut Command) -> std::io::Result<Self> {
```

And a few lines further down, change:

```rust
    fn status(&mut self) -> std::io::Result<Option<ExitStatus>> {
```

to:

```rust
    pub(super) fn status(&mut self) -> std::io::Result<Option<ExitStatus>> {
```

and:

```rust
    fn terminate(&mut self) {
```

to:

```rust
    pub(super) fn terminate(&mut self) {
```

(`status()` needs to stay visible too because `terminate()`'s body calls it, and both are now called from outside the struct's own `impl` block via the new module.)

- [ ] **Step 5: Run the existing test suite to confirm nothing broke**

Run: `cargo test --all-targets 2>&1 | tail -20`
Expected: all tests still pass (this is a pure visibility change, no behavior change).

- [ ] **Step 6: Commit the visibility change**

```bash
git add src/runtime/native.rs
git commit -m "refactor: widen ManagedChild visibility for reuse by the Wayland backend"
```

- [ ] **Step 7: Create the new module with just enough to compile, and its first (fast, no-sway) test**

Create `src/runtime/wayland.rs`:

```rust
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::native::ManagedChild;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_DISPLAY_DIMENSION: u16 = 4096;
const IPC_MAGIC: &[u8; 6] = b"i3-ipc";
const IPC_HEADER_LEN: usize = 14;
const IPC_RUN_COMMAND: u32 = 0;

static COMPOSITOR_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub struct WaylandCompositor {
    process: ManagedChild,
    runtime_dir: PathBuf,
    wayland_socket: String,
    ipc_socket: PathBuf,
}

impl WaylandCompositor {
    pub fn start(width: u16, height: u16) -> Result<Self, WaylandError> {
        validate_size(width, height)?;
        todo_unreachable()
    }
}

fn validate_size(width: u16, height: u16) -> Result<(), WaylandError> {
    if width == 0 || height == 0 || width > MAX_DISPLAY_DIMENSION || height > MAX_DISPLAY_DIMENSION {
        return Err(WaylandError::InvalidDisplaySize { width, height });
    }
    Ok(())
}

fn todo_unreachable() -> ! {
    unimplemented!("filled in by the next step")
}

#[derive(Debug)]
pub enum WaylandError {
    InvalidDisplaySize { width: u16, height: u16 },
}

impl Display for WaylandError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDisplaySize { width, height } => {
                write!(formatter, "invalid Wayland output size {width}x{height}")
            }
        }
    }
}

impl Error for WaylandError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_display_size_without_starting_sway() {
        assert!(matches!(
            WaylandCompositor::start(0, 100),
            Err(WaylandError::InvalidDisplaySize { .. })
        ));
        assert!(matches!(
            WaylandCompositor::start(MAX_DISPLAY_DIMENSION + 1, 100),
            Err(WaylandError::InvalidDisplaySize { .. })
        ));
    }
}
```

This is intentionally not finished yet (`todo_unreachable()` is a private placeholder that only exists to make Step 8 compile and pass without pulling in the rest of the implementation early — it is removed in Step 10, before this task is done, and this file does not compile into the final binary in this state — do not commit after this step).

- [ ] **Step 8: Wire the new module in so it compiles, then run the new test**

In `src/runtime/mod.rs`, add:

```rust
mod wayland;
```

next to the other `mod` declarations, and:

```rust
pub use wayland::{WaylandCompositor, WaylandError};
```

next to the other `pub use` lines.

Run: `cargo test --lib runtime::wayland::tests::rejects_invalid_display_size_without_starting_sway -- --exact --nocapture 2>&1 | tail -20`
Expected: **PASS** — the validation logic is real (Step 7 already wrote it correctly), only the success path is a stub. This step confirms the error path works before building the harder success path around it.

- [ ] **Step 9: Write the ignored integration test for the real success path**

Add to the `tests` module in `src/runtime/wayland.rs` (after the test from Step 7):

```rust
    #[test]
    #[ignore = "requires sway"]
    fn starts_a_headless_compositor_and_resizes_it() {
        let mut compositor = WaylandCompositor::start(800, 600).unwrap();
        assert_eq!(compositor.wayland_socket_name(), "wayland-1");
        assert!(
            compositor
                .runtime_dir()
                .join(compositor.wayland_socket_name())
                .exists()
        );

        compositor.resize(1024, 768).unwrap();

        let outputs = compositor.query_outputs().unwrap();
        let headless = outputs
            .iter()
            .find(|output| output["name"] == "HEADLESS-1")
            .expect("HEADLESS-1 output should exist");
        assert_eq!(headless["current_mode"]["width"], 1024);
        assert_eq!(headless["current_mode"]["height"], 768);
    }
```

This references `compositor.query_outputs()`, which doesn't exist yet — that's expected, it's part of what Step 10 implements (this test is written first, per TDD, and will fail to compile until then).

- [ ] **Step 10: Implement the real `start`, `resize`, IPC helpers, `query_outputs`, and `Drop`**

Replace the entire contents of `src/runtime/wayland.rs` with:

```rust
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::native::ManagedChild;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_DISPLAY_DIMENSION: u16 = 4096;
const IPC_MAGIC: &[u8; 6] = b"i3-ipc";
const IPC_HEADER_LEN: usize = 14;
const IPC_RUN_COMMAND: u32 = 0;
const IPC_GET_OUTPUTS: u32 = 3;
const SWAY_CONFIG: &str = "xwayland disable\nbar {\n  swaybar_command /bin/true\n}\n";

static COMPOSITOR_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub struct WaylandCompositor {
    process: ManagedChild,
    runtime_dir: PathBuf,
    wayland_socket: String,
    ipc_socket: PathBuf,
}

impl WaylandCompositor {
    pub fn start(width: u16, height: u16) -> Result<Self, WaylandError> {
        validate_size(width, height)?;

        let id = COMPOSITOR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let runtime_dir = std::env::temp_dir().join(format!(
            "microbox-wayland-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&runtime_dir).map_err(WaylandError::RuntimeDir)?;
        fs::set_permissions(&runtime_dir, fs::Permissions::from_mode(0o700))
            .map_err(WaylandError::RuntimeDir)?;

        let config_path = runtime_dir.join("sway.conf");
        fs::write(&config_path, SWAY_CONFIG).map_err(WaylandError::RuntimeDir)?;

        let mut command = Command::new("sway");
        command
            .arg("-c")
            .arg(&config_path)
            .env("WLR_BACKENDS", "headless")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let process = ManagedChild::spawn(&mut command).map_err(WaylandError::StartSway)?;

        let wayland_socket = "wayland-1".to_string();
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let ipc_socket = loop {
            if runtime_dir.join(&wayland_socket).exists() {
                if let Some(path) = find_ipc_socket(&runtime_dir) {
                    break path;
                }
            }
            if Instant::now() >= deadline {
                return Err(WaylandError::StartupTimeout(STARTUP_TIMEOUT));
            }
            thread::sleep(Duration::from_millis(25));
        };

        let mut compositor = Self {
            process,
            runtime_dir,
            wayland_socket,
            ipc_socket,
        };
        compositor.resize(width, height)?;
        Ok(compositor)
    }

    pub fn wayland_socket_name(&self) -> &str {
        &self.wayland_socket
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    pub fn resize(&mut self, width: u16, height: u16) -> Result<(), WaylandError> {
        validate_size(width, height)?;
        let command = format!("output HEADLESS-1 resolution {width}x{height}");
        let reply = ipc_roundtrip(&self.ipc_socket, IPC_RUN_COMMAND, command.as_bytes())?;
        let results: serde_json::Value =
            serde_json::from_slice(&reply).map_err(WaylandError::InvalidReply)?;
        let succeeded = results.as_array().is_some_and(|entries| {
            entries.iter().all(|entry| {
                entry
                    .get("success")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            })
        });
        if !succeeded {
            return Err(WaylandError::CommandFailed(results.to_string()));
        }
        Ok(())
    }

    pub fn query_outputs(&self) -> Result<Vec<serde_json::Value>, WaylandError> {
        let reply = ipc_roundtrip(&self.ipc_socket, IPC_GET_OUTPUTS, &[])?;
        let outputs: Vec<serde_json::Value> =
            serde_json::from_slice(&reply).map_err(WaylandError::InvalidReply)?;
        Ok(outputs)
    }
}

impl Drop for WaylandCompositor {
    fn drop(&mut self) {
        self.process.terminate();
        let _ = fs::remove_dir_all(&self.runtime_dir);
    }
}

fn validate_size(width: u16, height: u16) -> Result<(), WaylandError> {
    if width == 0 || height == 0 || width > MAX_DISPLAY_DIMENSION || height > MAX_DISPLAY_DIMENSION {
        return Err(WaylandError::InvalidDisplaySize { width, height });
    }
    Ok(())
}

fn find_ipc_socket(runtime_dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(runtime_dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("sway-ipc.") && name.ends_with(".sock") {
            return Some(entry.path());
        }
    }
    None
}

fn ipc_roundtrip(
    socket_path: &Path,
    message_type: u32,
    payload: &[u8],
) -> Result<Vec<u8>, WaylandError> {
    let mut stream = UnixStream::connect(socket_path).map_err(WaylandError::Ipc)?;

    let mut request = Vec::with_capacity(IPC_HEADER_LEN + payload.len());
    request.extend_from_slice(IPC_MAGIC);
    request.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
    request.extend_from_slice(&message_type.to_ne_bytes());
    request.extend_from_slice(payload);
    stream.write_all(&request).map_err(WaylandError::Ipc)?;

    let mut header = [0u8; IPC_HEADER_LEN];
    stream.read_exact(&mut header).map_err(WaylandError::Ipc)?;
    if &header[0..6] != IPC_MAGIC {
        return Err(WaylandError::Ipc(io::Error::new(
            io::ErrorKind::InvalidData,
            "reply did not start with the sway-ipc magic bytes",
        )));
    }
    let reply_length = u32::from_ne_bytes(header[6..10].try_into().unwrap()) as usize;
    let mut reply = vec![0u8; reply_length];
    stream.read_exact(&mut reply).map_err(WaylandError::Ipc)?;
    Ok(reply)
}

#[derive(Debug)]
pub enum WaylandError {
    InvalidDisplaySize { width: u16, height: u16 },
    RuntimeDir(io::Error),
    StartSway(io::Error),
    StartupTimeout(Duration),
    Ipc(io::Error),
    InvalidReply(serde_json::Error),
    CommandFailed(String),
}

impl Display for WaylandError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDisplaySize { width, height } => {
                write!(formatter, "invalid Wayland output size {width}x{height}")
            }
            Self::RuntimeDir(error) => {
                write!(formatter, "could not prepare the compositor runtime dir: {error}")
            }
            Self::StartSway(error) => write!(formatter, "could not start sway: {error}"),
            Self::StartupTimeout(timeout) => write!(
                formatter,
                "sway did not become ready within {:.1}s",
                timeout.as_secs_f32()
            ),
            Self::Ipc(error) => write!(formatter, "sway IPC error: {error}"),
            Self::InvalidReply(error) => {
                write!(formatter, "could not parse sway IPC reply: {error}")
            }
            Self::CommandFailed(reply) => write!(formatter, "sway command failed: {reply}"),
        }
    }
}

impl Error for WaylandError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RuntimeDir(error) | Self::StartSway(error) | Self::Ipc(error) => Some(error),
            Self::InvalidReply(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_display_size_without_starting_sway() {
        assert!(matches!(
            WaylandCompositor::start(0, 100),
            Err(WaylandError::InvalidDisplaySize { .. })
        ));
        assert!(matches!(
            WaylandCompositor::start(MAX_DISPLAY_DIMENSION + 1, 100),
            Err(WaylandError::InvalidDisplaySize { .. })
        ));
    }

    #[test]
    #[ignore = "requires sway"]
    fn starts_a_headless_compositor_and_resizes_it() {
        let mut compositor = WaylandCompositor::start(800, 600).unwrap();
        assert_eq!(compositor.wayland_socket_name(), "wayland-1");
        assert!(
            compositor
                .runtime_dir()
                .join(compositor.wayland_socket_name())
                .exists()
        );

        compositor.resize(1024, 768).unwrap();

        let outputs = compositor.query_outputs().unwrap();
        let headless = outputs
            .iter()
            .find(|output| output["name"] == "HEADLESS-1")
            .expect("HEADLESS-1 output should exist");
        assert_eq!(headless["current_mode"]["width"], 1024);
        assert_eq!(headless["current_mode"]["height"], 768);
    }
}
```

- [ ] **Step 11: Install sway locally and run the ignored test**

If `sway` is not already installed: `sudo apt-get install -y sway` (Debian/Ubuntu) or the equivalent for your distro.

Run: `cargo test --lib runtime::wayland::tests:: -- --ignored --exact --nocapture 2>&1 | tail -30`
Expected: `starts_a_headless_compositor_and_resizes_it` **PASSES**. If it fails on `WaylandCompositor::start` with a startup timeout, run `sway -c <path printed by the test failure, or rerun with a temporary eprintln of runtime_dir>` manually outside the test to see sway's own stderr (the struct redirects it to `Stdio::null()` for the library code, but you can temporarily change that to `Stdio::inherit()` locally while debugging — revert before committing).

- [ ] **Step 12: Run the full non-ignored suite, fmt, and clippy**

```bash
cargo test --all-targets 2>&1 | tail -20
cargo fmt --check
cargo clippy --all-targets -- -D warnings 2>&1 | tail -30
```

Expected: everything passes/clean. Fix anything clippy flags (e.g. it may prefer `is_some_and` in a slightly different form, or flag the `Vec::with_capacity` size hint — adjust as it suggests, these are mechanical).

- [ ] **Step 13: Add `sway` to CI so the ignored test actually runs there**

In `.github/workflows/ci.yml`, find the `linux` job's existing dependency-install step:

```yaml
      - name: Install X11 smoke-test dependencies
        run: sudo apt-get update && sudo apt-get install -y xvfb x11-apps x11-utils
```

Change it to also install sway:

```yaml
      - name: Install X11 smoke-test dependencies
        run: sudo apt-get update && sudo apt-get install -y xvfb x11-apps x11-utils sway
```

Then find:

```yaml
      - name: Xvfb integration tests
        run: |
          cargo test runtime::native::tests:: -- --ignored
```

and add a second step right after it:

```yaml
      - name: Wayland compositor integration tests
        run: |
          cargo test runtime::wayland::tests:: -- --ignored
```

- [ ] **Step 14: Commit the finished module, CI wiring, and module exports together**

```bash
git add src/runtime/wayland.rs src/runtime/mod.rs .github/workflows/ci.yml
git commit -m "add: headless Wayland compositor lifecycle (WaylandCompositor)"
```

- [ ] **Step 15: Push and confirm CI is green**

```bash
git push origin main
gh run list --branch main --limit 1
```

Then watch the run (`gh run watch <run-id> --exit-status`) and confirm both the `linux` job's new "Wayland compositor integration tests" step and the rest of the matrix pass.

---

## Self-review notes (already applied above)

- **Spec coverage for this phase:** the design spec's "Architecture" table row for `Xvfb` → `WaylandCompositor` is fully covered (spawn, ready-wait, resize, teardown). The `WaylandDisplay` row (capture/input/resize-integration/clipboard/window discovery) and the `NativeSession` backend-variant wiring are explicitly **not** in this phase — see "Next phases" below.
- **Placeholder scan:** the one intentional stub (`todo_unreachable()` in Step 7) is scoped to a single intermediate step and is fully replaced by Step 10, before the task is committed as done; every other step has complete, runnable code.
- **Type consistency:** `WaylandCompositor::start`/`resize` both return `Result<_, WaylandError>`; `query_outputs` returns `Result<Vec<serde_json::Value>, WaylandError>` and is used with that exact shape in the Step 9 test written before Step 10 defines it — confirmed matching.

## Next phases (not in this plan — brainstorm + plan separately once this lands)

1. `WaylandDisplay`: connect a `wayland-client` to the compositor's `wayland_socket_name()`/`runtime_dir()`, implement capture via `wlr-screencopy-unstable-v1`.
2. Input injection via `wlr-virtual-pointer-unstable-v1` + `zwp-virtual-keyboard-v1` (crate: `wayland-protocols-misc`).
3. Clipboard via `wlr-data-control-unstable-v1`.
4. Wire a `Wayland` variant into `NativeSession` alongside today's `X11` path, decide the `--runtime` CLI surface for selecting it (open question 1 in the spec).
5. A `firecrab-wayland`-style example Dockerfile once there's a real Wayland-only app to demonstrate it against.

## Verification notes

Everything in "Global Constraints" above was confirmed hands-on in the session that wrote this plan, not taken from documentation alone:
- Installed `sway` via `apt-get` locally and via `apk add sway` inside a fresh `alpine:3.24` container.
- Reproduced the `--cap-add SYS_NICE` requirement from a real `Operation not permitted` failure in a plain `docker run`, isolated it via `getcap /usr/bin/sway` (showed `cap_sys_nice=ep`), and confirmed the fix with `docker run --cap-add SYS_NICE ...`.
- Started sway headless, queried it live with `swaymsg -t get_outputs` / `swaymsg output HEADLESS-1 resolution WxH` / `swaymsg -t get_tree`, and read a real client (`foot`) window's JSON shape in `get_tree`.
- Confirmed clean SIGTERM shutdown leaves no orphan processes.
- Confirmed the minimal config removes the only two error-level log lines that mattered (swaybar crashing, Xwayland-not-found spam); the remaining `drmGetDevices2 failed` line is benign (no GPU in a headless container, same class of harmless warning `Xvfb`/Chrome/Firefox already produce elsewhere in this project).
