//! Synchronous settings I/O: call outside audio workers and async executor threads.
//! Loading never writes. Only an explicit user Save should call `save`, including
//! when recovering from a corrupt or unsupported settings file.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::contracts::{ApiError, ErrorCode, Mode, Settings};

pub const MAX_SETTINGS_BYTES: usize = 16 * 1024;

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            mode: Mode::Send,
            capture_device_id: None,
            playback_device_id: None,
            peer_ip: String::new(),
            udp_port: 46000,
            buffer_ms: 30,
        }
    }
}

/// Windows: LocalAppData/Audioportal/settings.json; other platforms use their
/// standard local data directory. Failure to resolve a directory is an error.
pub fn settings_path() -> Result<PathBuf, ApiError> {
    dirs::data_local_dir()
        .map(|directory| directory.join("Audioportal").join("settings.json"))
        .ok_or_else(|| ApiError {
            code: ErrorCode::InternalError,
            message: "Could not locate the user's application-data directory".into(),
        })
}

/// Save validation allows incomplete setup. Start must separately validate the
/// peer's unicast IPv4 address and enumerate the selected role's active endpoint.
pub fn validate(settings: &Settings) -> Result<(), ApiError> {
    let problem = if settings.version != 1 {
        Some("Settings version must be 1")
    } else if settings.udp_port == 0 {
        Some("UDP port must be between 1 and 65535")
    } else if !(10..=100).contains(&settings.buffer_ms) || !settings.buffer_ms.is_multiple_of(5) {
        Some("Buffer must be 10–100 ms in 5 ms increments")
    } else if settings.peer_ip.len() > 64 {
        Some("Peer IP must not exceed 64 bytes")
    } else if [&settings.capture_device_id, &settings.playback_device_id]
        .into_iter()
        .flatten()
        .any(|id| id.is_empty() || id.len() > 1024)
    {
        Some("Device IDs must be null or contain 1–1024 bytes")
    } else {
        None
    };
    match problem {
        Some(message) => Err(ApiError {
            code: ErrorCode::InvalidSettings,
            message: message.into(),
        }),
        None => Ok(()),
    }
}

/// Parse a complete settings body. Schema/type errors are invalid_request;
/// supported-shape values that fail validation are invalid_settings.
pub fn parse(bytes: &[u8]) -> Result<Settings, ApiError> {
    if bytes.len() > MAX_SETTINGS_BYTES {
        return Err(ApiError {
            code: ErrorCode::InvalidRequest,
            message: "Settings JSON exceeds 16 KiB".into(),
        });
    }
    let settings = serde_json::from_slice(bytes).map_err(|_| ApiError {
        code: ErrorCode::InvalidRequest,
        message: "Settings must be complete JSON matching the settings schema".into(),
    })?;
    validate(&settings)?;
    Ok(settings)
}

pub fn load(path: &Path) -> Result<Settings, ApiError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(error) => return Err(io_error("open settings", error)),
    };
    let mut bytes = Vec::new();
    file.take((MAX_SETTINGS_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("read settings", error))?;
    parse(&bytes)
}

/// Explicitly save (or replace corrupt settings). Validate before touching disk.
pub fn save(path: &Path, settings: &Settings) -> Result<(), ApiError> {
    validate(settings)?;
    let bytes = serde_json::to_vec(settings).map_err(|error| ApiError {
        code: ErrorCode::InternalError,
        message: format!("Could not serialize settings: {error}"),
    })?;
    // JSON escaping can increase the encoded size, so check the final body too.
    parse(&bytes)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|error| io_error("create settings directory", error))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| io_error("create temporary settings file", error))?;
    temporary
        .write_all(&bytes)
        .map_err(|error| io_error("write settings", error))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| io_error("flush settings", error))?;
    // tempfile uses MoveFileExW(MOVEFILE_REPLACE_EXISTING) on Windows and rename
    // on Unix. The destination is never removed first; failed temps are cleaned up.
    temporary
        .persist(path)
        .map_err(|error| io_error("replace settings", error.error))?;
    Ok(())
}

fn io_error(operation: &str, error: io::Error) -> ApiError {
    ApiError {
        code: ErrorCode::InternalError,
        message: format!("Could not {operation}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const FIXTURE: &[u8] = include_bytes!("../specs/fixtures/settings.json");

    #[test]
    fn defaults_round_trip_and_replace_existing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/settings.json");
        assert_eq!(load(&path).unwrap(), Settings::default());
        assert!(!path.exists());
        assert_eq!(
            serde_json::to_value(Settings::default()).unwrap(),
            json!({
                "version": 1, "mode": "send", "capture_device_id": null,
                "playback_device_id": null, "peer_ip": "", "udp_port": 46000, "buffer_ms": 30
            })
        );
        save(&path, &Settings::default()).unwrap();
        let settings = parse(FIXTURE).unwrap();
        save(&path, &settings).unwrap();
        assert_eq!(load(&path).unwrap(), settings);
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
            serde_json::from_slice::<Value>(FIXTURE).unwrap()
        );
        for peer_ip in ["", "unfinished address", "0.0.0.0", "240.1.2.3"] {
            let incomplete = Settings {
                peer_ip: peer_ip.into(),
                ..Settings::default()
            };
            save(&path, &incomplete).unwrap();
            assert_eq!(load(&path).unwrap(), incomplete);
        }
    }

    #[test]
    fn validation_and_corrupt_input_preserve_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let fixture: Value = serde_json::from_slice(FIXTURE).unwrap();
        for (field, value) in [
            ("version", json!(2)),
            ("version", json!("1")),
            ("mode", json!("duplex")),
            ("extra", json!(true)),
            ("udp_port", json!(0)),
            ("udp_port", json!(65536)),
            ("udp_port", json!(-1)),
            ("udp_port", json!(1.5)),
            ("buffer_ms", json!(5)),
            ("buffer_ms", json!(105)),
            ("buffer_ms", json!(31)),
            ("peer_ip", json!(null)),
            ("peer_ip", json!("é".repeat(33))),
            ("capture_device_id", json!("")),
            ("playback_device_id", json!("x".repeat(1025))),
        ] {
            let mut invalid = fixture.clone();
            invalid[field] = value;
            let bytes = serde_json::to_vec(&invalid).unwrap();
            assert!(parse(&bytes).is_err(), "{field}");
            fs::write(&path, &bytes).unwrap();
            assert!(load(&path).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
            // Typed callers cannot bypass validation when replacing any file.
            if let Ok(settings) = serde_json::from_value(invalid) {
                assert!(save(&path, &settings).is_err());
                assert_eq!(fs::read(&path).unwrap(), bytes);
            }
        }
        for field in fixture.as_object().unwrap().keys() {
            let mut missing = fixture.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(parse(&serde_json::to_vec(&missing).unwrap()).is_err());
        }
        for bytes in [b"{broken".to_vec(), vec![b' '; MAX_SETTINGS_BYTES + 1]] {
            fs::write(&path, &bytes).unwrap();
            assert!(load(&path).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        // Recovery requires an explicit valid Save.
        save(&path, &Settings::default()).unwrap();
        let original = fs::read(&path).unwrap();
        let invalid = Settings {
            udp_port: 0,
            ..Settings::default()
        };
        assert_eq!(
            save(&path, &invalid).unwrap_err().code,
            ErrorCode::InvalidSettings
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        for buffer_ms in [10, 100] {
            for udp_port in [1, 65535] {
                validate(&Settings {
                    buffer_ms,
                    udp_port,
                    peer_ip: "x".repeat(64),
                    capture_device_id: Some("x".repeat(1024)),
                    ..Settings::default()
                })
                .unwrap();
            }
        }
    }

    #[test]
    fn failed_write_preserves_previous_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        save(&path, &Settings::default()).unwrap();
        let original = fs::read(&path).unwrap();
        #[cfg(unix)]
        let permissions = {
            use std::os::unix::fs::PermissionsExt;
            let original = fs::metadata(directory.path()).unwrap().permissions();
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500)).unwrap();
            original
        };
        #[cfg(windows)]
        let locked = {
            use std::os::windows::fs::OpenOptionsExt;
            // Deny delete sharing: temp writing succeeds, replacement must fail.
            fs::OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(&path)
                .unwrap()
        };
        let result = save(&path, &parse(FIXTURE).unwrap());
        #[cfg(unix)]
        fs::set_permissions(directory.path(), permissions).unwrap();
        #[cfg(windows)]
        drop(locked);
        assert_eq!(result.unwrap_err().code, ErrorCode::InternalError);
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
