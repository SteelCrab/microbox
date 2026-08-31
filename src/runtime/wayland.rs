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
        let runtime_dir =
            std::env::temp_dir().join(format!("microbox-wayland-{}-{id}", std::process::id()));
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
    if width == 0 || height == 0 || width > MAX_DISPLAY_DIMENSION || height > MAX_DISPLAY_DIMENSION
    {
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
                write!(
                    formatter,
                    "could not prepare the compositor runtime dir: {error}"
                )
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
