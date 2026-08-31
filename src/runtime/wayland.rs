use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::native::ManagedChild;

const SWAY_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
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
    output_name: String,
}

impl WaylandCompositor {
    pub fn start(width: u16, height: u16) -> Result<Self, WaylandError> {
        validate_size(width, height)?;

        let id = COMPOSITOR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let runtime_dir =
            std::env::temp_dir().join(format!("microbox-wayland-{}-{id}", std::process::id()));
        // sway derives its IPC socket path as
        // "$XDG_RUNTIME_DIR/sway-ipc.<uid>.<pid>.sock", which has to fit in
        // sockaddr_un::sun_path (~108 bytes on Linux, including the NUL
        // terminator). A long TMPDIR/temp_dir() here eats directly into that
        // budget: sway aborts within milliseconds of startup if it doesn't
        // fit (see WaylandError::SwayExited below).
        //
        // DirBuilder::create (unlike create_dir_all) is a plain mkdir(2): it
        // does not follow a symlink at the final path component, and it
        // fails with AlreadyExists instead of silently succeeding if the
        // path is already occupied. That closes both a stale-directory reuse
        // hole (a recycled pid landing on a leftover directory from a killed
        // run, since `seq` restarts at 1 each process run) and a
        // symlink-planting hole in shared /tmp. Setting the mode via the
        // builder also makes directory creation and permissioning atomic,
        // instead of leaving a brief window at the default mode between a
        // separate create + chmod.
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&runtime_dir)
            .map_err(WaylandError::RuntimeDir)?;

        match Self::start_in(runtime_dir.clone(), width, height) {
            Ok(compositor) => Ok(compositor),
            Err(error) => {
                let _ = fs::remove_dir_all(&runtime_dir);
                Err(error)
            }
        }
    }

    fn start_in(runtime_dir: PathBuf, width: u16, height: u16) -> Result<Self, WaylandError> {
        let config_path = runtime_dir.join("sway.conf");
        fs::write(&config_path, SWAY_CONFIG).map_err(WaylandError::RuntimeDir)?;

        // Off by default: sway's own log output is otherwise discarded,
        // which is fine in normal operation but makes startup failures like
        // WaylandError::SwayExited hard to diagnose. Set
        // MICROBOX_SWAY_LOG=1 to see it.
        let sway_stderr = if std::env::var_os("MICROBOX_SWAY_LOG").is_some() {
            Stdio::inherit()
        } else {
            Stdio::null()
        };

        let mut command = Command::new("sway");
        command
            .arg("-c")
            .arg(&config_path)
            .env("WLR_BACKENDS", "headless")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(sway_stderr);
        let mut process = ManagedChild::spawn(&mut command).map_err(WaylandError::StartSway)?;

        let wayland_socket = "wayland-1".to_string();
        // Match the exact socket sway derives for the process we just
        // spawned, rather than the first sway-ipc.*.sock found in the
        // directory (non-deterministic if a stale one is somehow present).
        let ipc_socket = runtime_dir.join(format!(
            "sway-ipc.{}.{}.sock",
            nix::unistd::Uid::current().as_raw(),
            process.id()
        ));

        let deadline = Instant::now() + SWAY_STARTUP_TIMEOUT;
        loop {
            if let Some(status) = process.status().map_err(WaylandError::StartSway)? {
                return Err(WaylandError::SwayExited(status));
            }
            if runtime_dir.join(&wayland_socket).exists() && ipc_socket.exists() {
                break;
            }
            if Instant::now() >= deadline {
                return Err(WaylandError::StartupTimeout(SWAY_STARTUP_TIMEOUT));
            }
            thread::sleep(Duration::from_millis(25));
        }

        let mut compositor = Self {
            process,
            runtime_dir,
            wayland_socket,
            ipc_socket,
            output_name: String::new(),
        };

        let outputs = compositor.query_outputs()?;
        compositor.output_name = outputs
            .first()
            .and_then(|output| output.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or(WaylandError::NoHeadlessOutput)?;

        compositor.resize(width, height)?;
        Ok(compositor)
    }

    pub fn wayland_socket_name(&self) -> &str {
        &self.wayland_socket
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    pub fn output_name(&self) -> &str {
        &self.output_name
    }

    pub fn resize(&mut self, width: u16, height: u16) -> Result<(), WaylandError> {
        validate_size(width, height)?;
        let command = format!("output {} resolution {width}x{height}", self.output_name);
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

        // sway reports success for `output <name> resolution ...` even when
        // no output by that name currently exists: it just stores the
        // config for a future output with that name. Confirm the resize
        // actually took effect on the real output instead of trusting the
        // success flag alone.
        let outputs = self.query_outputs()?;
        let applied = outputs
            .iter()
            .find(|output| output["name"] == self.output_name)
            .is_some_and(|output| {
                output["current_mode"]["width"] == width
                    && output["current_mode"]["height"] == height
            });
        if !applied {
            return Err(WaylandError::ResizeNotApplied { width, height });
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
    if width == 0 || height == 0 || width > MAX_DISPLAY_DIMENSION || height > MAX_DISPLAY_DIMENSION
    {
        return Err(WaylandError::InvalidDisplaySize { width, height });
    }
    Ok(())
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
    SwayExited(ExitStatus),
    StartupTimeout(Duration),
    Ipc(io::Error),
    InvalidReply(serde_json::Error),
    CommandFailed(String),
    NoHeadlessOutput,
    ResizeNotApplied { width: u16, height: u16 },
}

impl Display for WaylandError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDisplaySize { width, height } => {
                write!(formatter, "invalid Wayland output size {width}x{height}")
            }
            Self::RuntimeDir(error) => {
                write!(
                    formatter,
                    "could not prepare the compositor runtime dir: {error}"
                )
            }
            Self::StartSway(error) => write!(formatter, "could not start sway: {error}"),
            Self::SwayExited(status) => {
                write!(formatter, "sway exited during startup ({status})")
            }
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
            Self::NoHeadlessOutput => {
                write!(formatter, "sway did not report any output")
            }
            Self::ResizeNotApplied { width, height } => write!(
                formatter,
                "sway did not apply the requested {width}x{height} resize (output missing or mode mismatch)"
            ),
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
    fn refuses_to_reuse_an_existing_runtime_dir() {
        // Predict the exact runtime_dir path that start() is about to try
        // to create (pid + the next COMPOSITOR_SEQUENCE value) and occupy
        // it first, the way a stale directory from a killed run (or a
        // planted symlink) would. This test never spawns sway: the
        // directory creation happens, and must fail, before sway is
        // spawned at all.
        let next_id = COMPOSITOR_SEQUENCE.load(Ordering::Relaxed);
        let runtime_dir =
            std::env::temp_dir().join(format!("microbox-wayland-{}-{next_id}", std::process::id()));
        fs::create_dir_all(&runtime_dir).unwrap();

        let result = WaylandCompositor::start(800, 600);

        match &result {
            Err(WaylandError::RuntimeDir(error))
                if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(other) => panic!("expected RuntimeDir(AlreadyExists), got Err({other:?})"),
            Ok(_) => panic!("expected RuntimeDir(AlreadyExists), got Ok(_)"),
        }
        // The pre-existing directory (not ours to touch) must be left alone.
        assert!(runtime_dir.exists());

        let _ = fs::remove_dir_all(&runtime_dir);
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
        // sway's headless backend currently names its single auto-created
        // output "HEADLESS-1"; this asserts the name we *discovered* via
        // GET_OUTPUTS matches that, without resize() itself ever hardcoding
        // the literal.
        assert_eq!(compositor.output_name(), "HEADLESS-1");

        compositor.resize(1024, 768).unwrap();

        let outputs = compositor.query_outputs().unwrap();
        let headless = outputs
            .iter()
            .find(|output| output["name"] == compositor.output_name())
            .expect("the discovered output should exist");
        assert_eq!(headless["current_mode"]["width"], 1024);
        assert_eq!(headless["current_mode"]["height"], 768);
    }

    #[test]
    #[ignore = "requires sway; run with a long TMPDIR to reproduce, e.g. \
                `TMPDIR=/tmp/$(python3 -c \"print('x'*80)\") cargo test --lib runtime::wayland::tests:: -- --ignored`"]
    fn reports_sway_exit_instead_of_timing_out_on_a_long_runtime_dir_path() {
        // Regression test for the finding: a long TMPDIR makes the derived
        // sway-ipc.<uid>.<pid>.sock path exceed sockaddr_un::sun_path's
        // ~108-byte limit, sway logs "Socket path won't fit into
        // ipc_sockaddr->sun_path" and aborts within milliseconds, but
        // wayland-1 was already created so the old filesystem-only
        // readiness loop spun out the full startup timeout instead of
        // reporting the real cause. Requires the caller to set a long
        // TMPDIR beforehand (see the #[ignore] reason).
        let start = Instant::now();
        let result = WaylandCompositor::start(800, 600);
        let elapsed = start.elapsed();

        match &result {
            Err(WaylandError::SwayExited(_)) => {}
            Err(other) => panic!(
                "expected SwayExited (make sure TMPDIR is long enough, per the #[ignore] reason), got Err({other:?})"
            ),
            Ok(_) => panic!(
                "expected SwayExited (make sure TMPDIR is long enough, per the #[ignore] reason), got Ok(_)"
            ),
        }
        assert!(
            elapsed < SWAY_STARTUP_TIMEOUT,
            "took {elapsed:?}, which suggests the timeout path fired instead of the exit-detection path"
        );
    }
}
