use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt::{self, Display, Formatter};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::protocol::InputEvent;
use crate::renderer::Frame;

use super::{NativeError, NativeSession};

static CONTAINER_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OciApplicationSpec {
    image: String,
    arguments: Vec<OsString>,
    engine: OsString,
}

impl OciApplicationSpec {
    pub fn new(image: impl Into<String>, arguments: impl IntoIterator<Item = OsString>) -> Self {
        Self {
            image: image.into(),
            arguments: arguments.into_iter().collect(),
            engine: OsString::from("docker"),
        }
    }

    pub fn image(&self) -> &str {
        &self.image
    }

    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    #[cfg(test)]
    fn with_engine(mut self, engine: impl Into<OsString>) -> Self {
        self.engine = engine.into();
        self
    }
}

pub struct OciSession {
    native: NativeSession,
    cleanup: ContainerCleanup,
}

impl OciSession {
    pub fn start(spec: &OciApplicationSpec, width: u16, height: u16) -> Result<Self, OciError> {
        verify_engine(&spec.engine)?;
        ensure_image(&spec.engine, &spec.image)?;

        let container_name = container_name();
        let cleanup = ContainerCleanup {
            engine: spec.engine.clone(),
            name: container_name.clone(),
            armed: true,
        };
        let native = NativeSession::start_with_command(
            width,
            height,
            OsStr::new(&spec.image),
            |display_name| docker_run_command(spec, &container_name, display_name),
        );

        match native {
            Ok(native) => Ok(Self { native, cleanup }),
            Err(error) => {
                drop(cleanup);
                Err(OciError::Session(error))
            }
        }
    }

    pub fn capture(&mut self) -> Result<Frame, OciError> {
        self.native.capture().map_err(OciError::Session)
    }

    pub fn frame_pending(&mut self) -> Result<bool, OciError> {
        self.native.frame_pending().map_err(OciError::Session)
    }

    pub fn inject(&self, event: &InputEvent) -> Result<(), OciError> {
        self.native.inject(event).map_err(OciError::Session)
    }

    pub fn display_size(&self) -> (u16, u16) {
        self.native.display_size()
    }

    pub fn is_running(&mut self) -> Result<bool, OciError> {
        self.native.is_running().map_err(OciError::Session)
    }

    pub fn container_name(&self) -> &str {
        &self.cleanup.name
    }
}

fn docker_run_command(spec: &OciApplicationSpec, name: &str, display_name: &str) -> Command {
    let socket = x11_socket_path(display_name);
    let mut command = Command::new(&spec.engine);
    command
        .arg("run")
        .arg("--rm")
        .arg("--name")
        .arg(name)
        .arg("--env")
        .arg(format!("DISPLAY={display_name}"))
        .arg("--env")
        .arg("GDK_BACKEND=x11")
        .arg("--volume")
        .arg(format!("{socket}:{socket}:rw"))
        .arg("--security-opt")
        .arg("no-new-privileges")
        .arg(&spec.image)
        .args(&spec.arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn x11_socket_path(display_name: &str) -> String {
    format!("/tmp/.X11-unix/X{}", display_name.trim_start_matches(':'))
}

fn verify_engine(engine: &OsStr) -> Result<(), OciError> {
    let output = Command::new(engine)
        .args(["info", "--format", "{{.ServerVersion}}"])
        .output()
        .map_err(|error| OciError::EngineUnavailable(error.to_string()))?;
    if output.status.success() {
        return Ok(());
    }
    Err(OciError::EngineUnavailable(output_message(&output.stderr)))
}

fn ensure_image(engine: &OsStr, image: &str) -> Result<(), OciError> {
    let inspect = Command::new(engine)
        .args(["image", "inspect", image])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| OciError::EngineUnavailable(error.to_string()))?;
    if inspect.success() {
        return Ok(());
    }

    let output = Command::new(engine)
        .args(["pull", image])
        .output()
        .map_err(|error| OciError::Pull {
            image: image.into(),
            message: error.to_string(),
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(OciError::Pull {
            image: image.into(),
            message: output_message(&output.stderr),
        })
    }
}

fn container_name() -> String {
    let sequence = CONTAINER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("micro-gui-{}-{sequence}", std::process::id())
}

fn output_message(bytes: &[u8]) -> String {
    let message = String::from_utf8_lossy(bytes);
    let message = message.trim();
    if message.is_empty() {
        "command failed without an error message".into()
    } else {
        const MAX_CHARS: usize = 2_000;
        message.chars().take(MAX_CHARS).collect()
    }
}

struct ContainerCleanup {
    engine: OsString,
    name: String,
    armed: bool,
}

impl Drop for ContainerCleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let _ = Command::new(&self.engine)
            .args(["rm", "--force", &self.name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        self.armed = false;
    }
}

#[derive(Debug)]
pub enum OciError {
    EngineUnavailable(String),
    Pull { image: String, message: String },
    Session(NativeError),
}

impl Display for OciError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::EngineUnavailable(message) => {
                write!(formatter, "Docker/OCI runtime is unavailable: {message}")
            }
            Self::Pull { image, message } => {
                write!(formatter, "could not pull OCI image '{image}': {message}")
            }
            Self::Session(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for OciError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Session(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_image_arguments_separate() {
        let spec = OciApplicationSpec::new("example/gui:1", [OsString::from("a file.png")]);
        assert_eq!(spec.image(), "example/gui:1");
        assert_eq!(spec.arguments(), [OsString::from("a file.png")]);
    }

    #[test]
    fn reports_missing_engine() {
        let spec =
            OciApplicationSpec::new("example/gui:1", []).with_engine("missing-micro-gui-engine");
        assert!(matches!(
            OciSession::start(&spec, 320, 180),
            Err(OciError::EngineUnavailable(_))
        ));
    }

    #[test]
    fn generated_container_names_are_distinct_and_scoped() {
        let first = container_name();
        let second = container_name();
        assert!(first.starts_with("micro-gui-"));
        assert_ne!(first, second);
    }

    #[test]
    fn maps_only_the_private_x11_socket() {
        assert_eq!(x11_socket_path(":42"), "/tmp/.X11-unix/X42");
    }
}
