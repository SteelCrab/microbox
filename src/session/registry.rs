use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::unistd::{Pid, geteuid};

const FORMAT_VERSION: &str = "1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: String,
    pub application: String,
    pub runtime: String,
    pub pid: u32,
    pub process_start_ticks: u64,
    pub started_unix_seconds: u64,
}

pub struct SessionRegistry {
    directory: PathBuf,
}

impl SessionRegistry {
    pub fn discover() -> Result<Self, RegistryError> {
        let directory = if let Some(path) = std::env::var_os("MICRO_GUI_RUNTIME_DIR") {
            PathBuf::from(path)
        } else if let Some(path) = std::env::var_os("XDG_RUNTIME_DIR") {
            PathBuf::from(path).join("micro-gui").join("sessions")
        } else {
            std::env::temp_dir()
                .join(format!("micro-gui-{}", geteuid().as_raw()))
                .join("sessions")
        };
        Self::at(directory)
    }

    fn at(directory: PathBuf) -> Result<Self, RegistryError> {
        fs::create_dir_all(&directory).map_err(RegistryError::Io)?;
        let metadata = fs::symlink_metadata(&directory).map_err(RegistryError::Io)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(RegistryError::UnsafeDirectory(directory));
        }
        if metadata.uid() != geteuid().as_raw() {
            return Err(RegistryError::WrongOwner(directory));
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(RegistryError::Io)?;
        Ok(Self { directory })
    }

    pub fn register(
        &self,
        application: impl Into<String>,
        runtime: impl Into<String>,
    ) -> Result<SessionRegistration, RegistryError> {
        let pid = std::process::id();
        let record = SessionRecord {
            id: format!("gui-{pid}"),
            application: application.into(),
            runtime: runtime.into(),
            pid,
            process_start_ticks: process_start_ticks(pid)?
                .ok_or(RegistryError::ProcessGone(pid))?,
            started_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| RegistryError::Clock(error.to_string()))?
                .as_secs(),
        };
        let path = self.record_path(&record.id)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(RegistryError::Io)?;
        file.write_all(serialize(&record).as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(RegistryError::Io)?;
        Ok(SessionRegistration { record, path })
    }

    pub fn list(&self) -> Result<Vec<SessionRecord>, RegistryError> {
        let mut records = Vec::new();
        for entry in fs::read_dir(&self.directory).map_err(RegistryError::Io)? {
            let entry = entry.map_err(RegistryError::Io)?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("session") {
                continue;
            }
            let record = match read_record(&path) {
                Ok(record) => record,
                Err(_) => continue,
            };
            if process_matches(&record)? {
                records.push(record);
            } else {
                let _ = fs::remove_file(path);
            }
        }
        records.sort_by_key(|record| record.started_unix_seconds);
        Ok(records)
    }

    pub fn stop(&self, id: &str) -> Result<SessionRecord, RegistryError> {
        let path = self.record_path(id)?;
        if !path.exists() {
            return Err(RegistryError::SessionNotFound(id.into()));
        }
        let record = read_record(&path)?;
        if !process_matches(&record)? {
            let _ = fs::remove_file(path);
            return Err(RegistryError::StaleSession(id.into()));
        }
        match kill(Pid::from_raw(record.pid as i32), Signal::SIGTERM) {
            Ok(()) => Ok(record),
            Err(Errno::ESRCH) => {
                let _ = fs::remove_file(path);
                Err(RegistryError::StaleSession(id.into()))
            }
            Err(error) => Err(RegistryError::Signal(error.to_string())),
        }
    }

    fn record_path(&self, id: &str) -> Result<PathBuf, RegistryError> {
        if id.is_empty()
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(RegistryError::InvalidId(id.into()));
        }
        Ok(self.directory.join(format!("{id}.session")))
    }
}

pub struct SessionRegistration {
    record: SessionRecord,
    path: PathBuf,
}

impl SessionRegistration {
    pub fn record(&self) -> &SessionRecord {
        &self.record
    }
}

impl Drop for SessionRegistration {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn process_matches(record: &SessionRecord) -> Result<bool, RegistryError> {
    Ok(process_start_ticks(record.pid)? == Some(record.process_start_ticks))
}

fn process_start_ticks(pid: u32) -> Result<Option<u64>, RegistryError> {
    let path = format!("/proc/{pid}/stat");
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(RegistryError::Io(error)),
    };
    let closing_parenthesis = contents.rfind(')').ok_or_else(|| {
        RegistryError::InvalidRecord(format!("invalid /proc/{pid}/stat contents"))
    })?;
    let start_ticks = contents[closing_parenthesis + 1..]
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| RegistryError::InvalidRecord(format!("missing start time for pid {pid}")))?
        .parse::<u64>()
        .map_err(|_| RegistryError::InvalidRecord(format!("invalid start time for pid {pid}")))?;
    Ok(Some(start_ticks))
}

fn serialize(record: &SessionRecord) -> String {
    format!(
        "version={FORMAT_VERSION}\nid={}\npid={}\nprocess_start_ticks={}\nstarted_unix_seconds={}\nruntime={}\napplication={}\n",
        record.id,
        record.pid,
        record.process_start_ticks,
        record.started_unix_seconds,
        hex_encode(record.runtime.as_bytes()),
        hex_encode(record.application.as_bytes())
    )
}

fn read_record(path: &Path) -> Result<SessionRecord, RegistryError> {
    let metadata = fs::symlink_metadata(path).map_err(RegistryError::Io)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != geteuid().as_raw()
    {
        return Err(RegistryError::UnsafeRecord(path.to_path_buf()));
    }
    let contents = fs::read_to_string(path).map_err(RegistryError::Io)?;
    let field = |name: &str| {
        contents
            .lines()
            .find_map(|line| {
                line.strip_prefix(name)
                    .and_then(|value| value.strip_prefix('='))
            })
            .ok_or_else(|| RegistryError::InvalidRecord(format!("missing '{name}'")))
    };
    if field("version")? != FORMAT_VERSION {
        return Err(RegistryError::InvalidRecord("unsupported version".into()));
    }
    let parse_number = |name: &str| {
        field(name)?
            .parse::<u64>()
            .map_err(|_| RegistryError::InvalidRecord(format!("invalid '{name}'")))
    };
    let pid = parse_number("pid")?;
    let pid = u32::try_from(pid)
        .map_err(|_| RegistryError::InvalidRecord("pid does not fit u32".into()))?;
    Ok(SessionRecord {
        id: field("id")?.into(),
        application: String::from_utf8(hex_decode(field("application")?)?)
            .map_err(|_| RegistryError::InvalidRecord("application is not UTF-8".into()))?,
        runtime: String::from_utf8(hex_decode(field("runtime")?)?)
            .map_err(|_| RegistryError::InvalidRecord("runtime is not UTF-8".into()))?,
        pid,
        process_start_ticks: parse_number("process_start_ticks")?,
        started_unix_seconds: parse_number("started_unix_seconds")?,
    })
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn hex_decode(encoded: &str) -> Result<Vec<u8>, RegistryError> {
    if encoded.len() % 2 != 0 {
        return Err(RegistryError::InvalidRecord("odd hex field length".into()));
    }
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_nibble(pair[0])?;
            let low = hex_nibble(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(byte: u8) -> Result<u8, RegistryError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(RegistryError::InvalidRecord("invalid hex field".into())),
    }
}

#[derive(Debug)]
pub enum RegistryError {
    Io(std::io::Error),
    Clock(String),
    UnsafeDirectory(PathBuf),
    WrongOwner(PathBuf),
    UnsafeRecord(PathBuf),
    InvalidRecord(String),
    InvalidId(String),
    SessionNotFound(String),
    ProcessGone(u32),
    StaleSession(String),
    Signal(String),
}

impl Display for RegistryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => Display::fmt(error, formatter),
            Self::Clock(message) => write!(formatter, "system clock error: {message}"),
            Self::UnsafeDirectory(path) => {
                write!(
                    formatter,
                    "unsafe session registry directory: {}",
                    path.display()
                )
            }
            Self::WrongOwner(path) => write!(
                formatter,
                "session registry is owned by another user: {}",
                path.display()
            ),
            Self::UnsafeRecord(path) => {
                write!(formatter, "unsafe session record: {}", path.display())
            }
            Self::InvalidRecord(message) => write!(formatter, "invalid session record: {message}"),
            Self::InvalidId(id) => write!(formatter, "invalid session id '{id}'"),
            Self::SessionNotFound(id) => write!(formatter, "session '{id}' was not found"),
            Self::ProcessGone(pid) => {
                write!(formatter, "session process {pid} is no longer running")
            }
            Self::StaleSession(id) => write!(formatter, "session '{id}' is no longer running"),
            Self::Signal(message) => write!(formatter, "could not stop session: {message}"),
        }
    }
}

impl Error for RegistryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_registry(label: &str) -> SessionRegistry {
        let directory = std::env::temp_dir().join(format!(
            "micro-gui-registry-test-{}-{label}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        SessionRegistry::at(directory).unwrap()
    }

    #[test]
    fn registers_lists_and_removes_current_process() {
        let registry = test_registry("lifecycle");
        let registration = registry.register("app with spaces", "native").unwrap();
        let listed = registry.list().unwrap();
        assert_eq!(listed, [registration.record().clone()]);
        drop(registration);
        assert!(registry.list().unwrap().is_empty());
        fs::remove_dir_all(registry.directory).unwrap();
    }

    #[test]
    fn rejects_path_traversal_ids() {
        let registry = test_registry("traversal");
        assert!(matches!(
            registry.stop("../other"),
            Err(RegistryError::InvalidId(_))
        ));
        fs::remove_dir_all(registry.directory).unwrap();
    }

    #[test]
    fn record_round_trip_handles_arbitrary_text() {
        let record = SessionRecord {
            id: "gui-1".into(),
            application: "한글\napp\tname".into(),
            runtime: "oci".into(),
            pid: 1,
            process_start_ticks: 2,
            started_unix_seconds: 3,
        };
        let registry = test_registry("roundtrip");
        let path = registry.record_path(&record.id).unwrap();
        fs::write(&path, serialize(&record)).unwrap();
        assert_eq!(read_record(&path).unwrap(), record);
        fs::remove_dir_all(registry.directory).unwrap();
    }
}
