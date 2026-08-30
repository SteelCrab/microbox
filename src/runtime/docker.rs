use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt::{self, Display, Formatter};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::protocol::AgentExit;
use crate::protocol::InputEvent;
use crate::renderer::Frame;

#[cfg(target_os = "linux")]
use super::NativeSession;
use super::{FirecrabError, FirecrabSession, NativeError};

static CONTAINER_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OciApplicationSpec {
    image: String,
    arguments: Vec<OsString>,
    engine: OsString,
    debug: bool,
}

impl OciApplicationSpec {
    pub fn new(image: impl Into<String>, arguments: impl IntoIterator<Item = OsString>) -> Self {
        Self {
            image: image.into(),
            arguments: arguments.into_iter().collect(),
            engine: OsString::from("docker"),
            debug: false,
        }
    }

    pub fn image(&self) -> &str {
        &self.image
    }

    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    #[cfg(test)]
    fn with_engine(mut self, engine: impl Into<OsString>) -> Self {
        self.engine = engine.into();
        self
    }
}

pub struct OciSession {
    backend: OciBackend,
    cleanup: ContainerCleanup,
}

enum OciBackend {
    #[cfg(target_os = "linux")]
    Native(Box<NativeSession>),
    Agent(FirecrabSession),
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
        let backend = start_backend(spec, &container_name, width, height);

        match backend {
            Ok(backend) => Ok(Self { backend, cleanup }),
            Err(error) => {
                drop(cleanup);
                Err(error)
            }
        }
    }

    pub fn start_agent(
        spec: &OciApplicationSpec,
        width: u16,
        height: u16,
    ) -> Result<Self, OciError> {
        verify_engine(&spec.engine)?;
        ensure_image(&spec.engine, &spec.image)?;

        let container_name = container_name();
        let cleanup = ContainerCleanup {
            engine: spec.engine.clone(),
            name: container_name.clone(),
            armed: true,
        };
        match start_agent_backend(spec, &container_name, width, height) {
            Ok(backend) => Ok(Self { backend, cleanup }),
            Err(error) => {
                drop(cleanup);
                Err(error)
            }
        }
    }

    pub fn capture(&mut self) -> Result<Frame, OciError> {
        match &mut self.backend {
            #[cfg(target_os = "linux")]
            OciBackend::Native(session) => session.capture().map_err(OciError::Session),
            OciBackend::Agent(session) => session.capture().map_err(OciError::Transport),
        }
    }

    pub fn frame_pending(&mut self) -> Result<bool, OciError> {
        match &mut self.backend {
            #[cfg(target_os = "linux")]
            OciBackend::Native(session) => session.frame_pending().map_err(OciError::Session),
            OciBackend::Agent(session) => session.frame_pending().map_err(OciError::Transport),
        }
    }

    pub fn inject(&mut self, event: &InputEvent) -> Result<(), OciError> {
        match &mut self.backend {
            #[cfg(target_os = "linux")]
            OciBackend::Native(session) => session.inject(event).map_err(OciError::Session),
            OciBackend::Agent(session) => session.inject(event).map_err(OciError::Transport),
        }
    }

    pub fn display_size(&self) -> (u16, u16) {
        match &self.backend {
            #[cfg(target_os = "linux")]
            OciBackend::Native(session) => session.display_size(),
            OciBackend::Agent(session) => session.display_size(),
        }
    }

    pub fn resize(&mut self, width: u16, height: u16) -> Result<(), OciError> {
        match &mut self.backend {
            #[cfg(target_os = "linux")]
            OciBackend::Native(session) => session.resize(width, height).map_err(OciError::Session),
            OciBackend::Agent(session) => {
                session.resize(width, height).map_err(OciError::Transport)
            }
        }
    }

    pub fn is_running(&mut self) -> Result<bool, OciError> {
        match &mut self.backend {
            #[cfg(target_os = "linux")]
            OciBackend::Native(session) => session.is_running().map_err(OciError::Session),
            OciBackend::Agent(session) => session.is_running().map_err(OciError::Transport),
        }
    }

    pub fn termination(&mut self) -> Result<Option<AgentExit>, OciError> {
        match &mut self.backend {
            #[cfg(target_os = "linux")]
            OciBackend::Native(session) => session
                .application_status()
                .map(|status| {
                    status.map(|status| {
                        AgentExit::status(
                            status.success(),
                            format!("OCI application exited with {status}"),
                        )
                    })
                })
                .map_err(OciError::Session),
            OciBackend::Agent(session) => Ok(session.termination().cloned()),
        }
    }

    pub fn debug_diagnostics(&self) -> Vec<String> {
        if matches!(
            &self.backend,
            OciBackend::Agent(session) if session.termination().is_some()
        ) {
            wait_for_container_stop(&self.cleanup.engine, &self.cleanup.name);
        }
        collect_container_diagnostics(&self.cleanup.engine, &self.cleanup.name)
    }

    pub fn container_name(&self) -> &str {
        &self.cleanup.name
    }
}

#[cfg(target_os = "linux")]
fn start_backend(
    spec: &OciApplicationSpec,
    container_name: &str,
    width: u16,
    height: u16,
) -> Result<OciBackend, OciError> {
    NativeSession::start_with_command(width, height, OsStr::new(&spec.image), |display_name| {
        docker_run_command(spec, container_name, display_name)
    })
    .map(Box::new)
    .map(OciBackend::Native)
    .map_err(OciError::Session)
}

#[cfg(target_os = "macos")]
fn start_backend(
    spec: &OciApplicationSpec,
    container_name: &str,
    width: u16,
    height: u16,
) -> Result<OciBackend, OciError> {
    start_agent_backend(spec, container_name, width, height)
}

fn start_agent_backend(
    spec: &OciApplicationSpec,
    container_name: &str,
    width: u16,
    height: u16,
) -> Result<OciBackend, OciError> {
    let token = random_token()?;
    let output = docker_agent_command(spec, container_name, &token)
        .output()
        .map_err(|error| OciError::StartAgent(error.to_string()))?;
    if !output.status.success() {
        return Err(OciError::StartAgent(output_message(&output.stderr)));
    }

    let endpoint = match published_agent_endpoint(&spec.engine, container_name) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            return Err(agent_unavailable_with_diagnostics(
                &spec.engine,
                container_name,
                error.to_string(),
                spec.debug,
            ));
        }
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last_error = None;
    while Instant::now() < deadline {
        match FirecrabSession::connect(&endpoint, &token, width, height) {
            Ok(session) => return Ok(OciBackend::Agent(session)),
            Err(error) => {
                let terminal_error = matches!(
                    error,
                    FirecrabError::Agent(_) | FirecrabError::Protocol(_) | FirecrabError::Wire(_)
                );
                last_error = Some(error.to_string());
                if terminal_error
                    || matches!(
                        container_running(&spec.engine, container_name),
                        Ok(Some(false))
                    )
                {
                    break;
                }
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(agent_unavailable_with_diagnostics(
        &spec.engine,
        container_name,
        last_error.unwrap_or_else(|| {
            "the image must start `microbox agent` and expose TCP port 5943".into()
        }),
        spec.debug,
    ))
}

#[cfg(target_os = "linux")]
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

fn docker_agent_command(spec: &OciApplicationSpec, name: &str, token: &str) -> Command {
    let mut command = Command::new(&spec.engine);
    command
        .arg("run")
        .arg("--detach")
        .arg("--name")
        .arg(name)
        .arg("--env")
        .arg(format!("MICROBOX_AGENT_TOKEN={token}"))
        .arg("--publish")
        .arg(format!(
            "127.0.0.1::{}",
            crate::protocol::DEFAULT_AGENT_PORT
        ))
        .arg("--security-opt")
        .arg("no-new-privileges");
    if spec.debug {
        command.arg("--env").arg("MICROBOX_DEBUG=1");
    }
    command
        .arg(&spec.image)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !spec.arguments.is_empty() {
        command.arg("--").args(&spec.arguments);
    }
    command
}

fn random_token() -> Result<String, OciError> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|error| OciError::Token(error.to_string()))?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        token.push(HEX[(byte >> 4) as usize] as char);
        token.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(token)
}

fn published_agent_endpoint(engine: &OsStr, name: &str) -> Result<String, OciError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last_message = String::new();
    while Instant::now() < deadline {
        let output = Command::new(engine)
            .args([
                "port",
                name,
                &format!("{}/tcp", crate::protocol::DEFAULT_AGENT_PORT),
            ])
            .output()
            .map_err(|error| OciError::Port(error.to_string()))?;
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(port) = parse_published_port(&text) {
                return Ok(format!("127.0.0.1:{port}"));
            }
            last_message = text.trim().into();
        } else {
            last_message = output_message(&output.stderr);
        }
        if matches!(container_running(engine, name), Ok(Some(false))) {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(OciError::Port(if last_message.is_empty() {
        "Docker did not publish the guest agent port".into()
    } else {
        last_message
    }))
}

fn parse_published_port(value: &str) -> Option<u16> {
    value
        .lines()
        .find_map(|line| line.trim().rsplit_once(':')?.1.parse().ok())
}

#[cfg(any(target_os = "linux", test))]
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
    format!("microbox-{}-{sequence}", std::process::id())
}

fn output_message(bytes: &[u8]) -> String {
    let message = String::from_utf8_lossy(bytes);
    let message = message.trim();
    if message.is_empty() {
        "command failed without an error message".into()
    } else {
        const MAX_CHARS: usize = 2_000;
        clean_diagnostic_text(&message.chars().take(MAX_CHARS).collect::<String>())
    }
}

fn agent_unavailable_with_diagnostics(
    engine: &OsStr,
    name: &str,
    message: String,
    debug: bool,
) -> OciError {
    if !debug {
        return OciError::AgentUnavailable(message);
    }
    let diagnostics = collect_container_diagnostics(engine, name);
    let details = if diagnostics.is_empty() {
        message
    } else {
        format!("{message}\n  {}", diagnostics.join("\n  "))
    };
    OciError::AgentUnavailable(details)
}

fn wait_for_container_stop(engine: &OsStr, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if !matches!(container_running(engine, name), Ok(Some(true))) {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn container_running(engine: &OsStr, name: &str) -> Result<Option<bool>, String> {
    let output = Command::new(engine)
        .args(["inspect", "--format", "{{.State.Running}}", name])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Ok(None);
    }
    match String::from_utf8_lossy(&output.stdout).trim() {
        "true" => Ok(Some(true)),
        "false" => Ok(Some(false)),
        value => Err(format!("Docker returned invalid running state {value:?}")),
    }
}

fn collect_container_diagnostics(engine: &OsStr, name: &str) -> Vec<String> {
    let mut diagnostics = vec![format!("oci container={name}")];
    let inspect = Command::new(engine)
        .args([
            "inspect",
            "--format",
            "status={{.State.Status}} running={{.State.Running}} exit_code={{.State.ExitCode}} oom_killed={{.State.OOMKilled}} error={{json .State.Error}} started_at={{.State.StartedAt}} finished_at={{.State.FinishedAt}}",
            name,
        ])
        .output();
    append_command_diagnostics(&mut diagnostics, "oci state", inspect);

    let logs = Command::new(engine)
        .args(["logs", "--timestamps", "--tail", "200", name])
        .output();
    match logs {
        Ok(output) => {
            append_log_stream(&mut diagnostics, "oci stdout", &output.stdout);
            append_log_stream(&mut diagnostics, "oci stderr", &output.stderr);
            if !output.status.success() {
                diagnostics.push(format!("oci logs command exited with {}", output.status));
            } else if output.stdout.is_empty() && output.stderr.is_empty() {
                diagnostics.push("oci logs=(empty)".into());
            }
        }
        Err(error) => diagnostics.push(format!("oci logs unavailable: {error}")),
    }
    diagnostics
}

fn append_command_diagnostics(
    diagnostics: &mut Vec<String>,
    label: &str,
    output: std::io::Result<std::process::Output>,
) {
    match output {
        Ok(output) if output.status.success() => {
            let value = clean_diagnostic_text(&String::from_utf8_lossy(&output.stdout));
            diagnostics.push(format!("{label}={}", value.trim()));
        }
        Ok(output) => {
            let error = output_message(&output.stderr);
            diagnostics.push(format!("{label} unavailable ({}): {error}", output.status));
        }
        Err(error) => diagnostics.push(format!("{label} unavailable: {error}")),
    }
}

fn append_log_stream(diagnostics: &mut Vec<String>, label: &str, bytes: &[u8]) {
    const MAX_LOG_CHARS: usize = 16_000;
    let text = String::from_utf8_lossy(bytes);
    let text: String = text.chars().take(MAX_LOG_CHARS).collect();
    for line in text.lines() {
        diagnostics.push(format!("{label}: {}", clean_diagnostic_text(line)));
    }
    if String::from_utf8_lossy(bytes).chars().count() > MAX_LOG_CHARS {
        diagnostics.push(format!(
            "{label}: (truncated after {MAX_LOG_CHARS} characters)"
        ));
    }
}

fn clean_diagnostic_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() && !matches!(character, '\t') {
                ' '
            } else {
                character
            }
        })
        .collect()
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
    Transport(FirecrabError),
    StartAgent(String),
    AgentUnavailable(String),
    Port(String),
    Token(String),
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
            Self::Transport(error) => Display::fmt(error, formatter),
            Self::StartAgent(message) => {
                write!(formatter, "could not start OCI GUI agent: {message}")
            }
            Self::AgentUnavailable(message) => write!(
                formatter,
                "OCI GUI agent is unavailable: {message}\n  hint: on macOS use an agent image such as examples/firecrab-xeyes"
            ),
            Self::Port(message) => write!(formatter, "could not resolve OCI agent port: {message}"),
            Self::Token(message) => {
                write!(formatter, "could not generate OCI agent token: {message}")
            }
        }
    }
}

impl Error for OciError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Session(error) => Some(error),
            Self::Transport(error) => Some(error),
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
            OciApplicationSpec::new("example/gui:1", []).with_engine("missing-microbox-engine");
        assert!(matches!(
            OciSession::start(&spec, 320, 180),
            Err(OciError::EngineUnavailable(_))
        ));
    }

    #[test]
    fn generated_container_names_are_distinct_and_scoped() {
        let first = container_name();
        let second = container_name();
        assert!(first.starts_with("microbox-"));
        assert_ne!(first, second);
    }

    #[test]
    fn maps_only_the_private_x11_socket() {
        assert_eq!(x11_socket_path(":42"), "/tmp/.X11-unix/X42");
    }

    #[test]
    fn builds_loopback_only_agent_container_command() {
        let spec = OciApplicationSpec::new("example/gui-agent:1", [OsString::from("--flag")]);
        let command = docker_agent_command(&spec, "microbox-test", "secret");
        let arguments = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--publish", "127.0.0.1::5943"])
        );
        assert!(arguments.contains(&"MICROBOX_AGENT_TOKEN=secret".into()));
        assert!(!arguments.contains(&"--rm".into()));
        assert!(arguments.windows(2).any(|pair| pair == ["--", "--flag"]));
    }

    #[test]
    fn enables_guest_debug_without_exposing_the_token() {
        let spec = OciApplicationSpec::new("example/gui-agent:1", []).with_debug(true);
        let command = docker_agent_command(&spec, "microbox-test", "secret");
        let arguments = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(arguments.contains(&"MICROBOX_DEBUG=1".into()));
        assert_eq!(
            arguments
                .iter()
                .filter(|argument| argument.contains("secret"))
                .count(),
            1
        );
    }

    #[test]
    fn strips_control_characters_from_docker_diagnostics() {
        assert_eq!(
            clean_diagnostic_text("safe\n\x1b[31m\ttext"),
            "safe  [31m\ttext"
        );
    }

    #[test]
    fn parses_ipv4_and_ipv6_docker_ports() {
        assert_eq!(parse_published_port("127.0.0.1:49153\n"), Some(49153));
        assert_eq!(parse_published_port("[::1]:49154\n"), Some(49154));
        assert_eq!(parse_published_port(""), None);
    }
}
