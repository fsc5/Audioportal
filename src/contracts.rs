//! JSON shapes shared by settings, the session controller, and the local API.
use serde::{Deserialize, Serialize};

// deserialize_with makes the field required; Option alone treats absence as null.
fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Send,
    Receive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Capture,
    Playback,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Devices {
    pub devices: Vec<Device>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub version: u8,
    pub mode: Mode,
    #[serde(deserialize_with = "required_nullable")]
    pub capture_device_id: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub playback_device_id: Option<String>,
    pub peer_ip: String,
    pub udp_port: u16,
    pub buffer_ms: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Stopped,
    Starting,
    Listening,
    Receiving,
    Sending,
    Stopping,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidSettings,
    InvalidRequest,
    LifecycleConflict,
    DeviceUnavailable,
    BackendUnavailable,
    WorkerFailed,
    InternalError,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    pub error: ApiError,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub status: Status,
    #[serde(deserialize_with = "required_nullable")]
    pub mode: Option<Mode>,
    pub input_peak: f64,
    pub output_peak: f64,
    pub packets_lost: u64,
    pub buffer_ms: u16,
    #[serde(deserialize_with = "required_nullable")]
    pub error: Option<ApiError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    pub csrf_token: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_round_trip() {
        for fixture in [
            include_str!("../specs/fixtures/devices.json"),
            include_str!("../specs/fixtures/settings.json"),
            include_str!("../specs/fixtures/state-stopped.json"),
            include_str!("../specs/fixtures/state-receiving.json"),
            include_str!("../specs/fixtures/error.json"),
        ] {
            let value: serde_json::Value = serde_json::from_str(fixture).unwrap();
            let encoded = if value.get("devices").is_some() {
                serde_json::to_value(serde_json::from_value::<Devices>(value.clone()).unwrap())
            } else if value.get("version").is_some() {
                serde_json::to_value(serde_json::from_value::<Settings>(value.clone()).unwrap())
            } else if value.get("status").is_some() {
                serde_json::to_value(serde_json::from_value::<State>(value.clone()).unwrap())
            } else {
                serde_json::to_value(
                    serde_json::from_value::<ErrorResponse>(value.clone()).unwrap(),
                )
            }
            .unwrap();
            assert_eq!(encoded, value);
        }
    }

    #[test]
    fn nullable_fields_require_explicit_null() {
        let mut settings: serde_json::Value =
            serde_json::from_str(include_str!("../specs/fixtures/settings.json")).unwrap();
        for field in ["capture_device_id", "playback_device_id"] {
            let original = settings[field].clone();
            settings.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<Settings>(settings.clone()).is_err());
            settings[field] = serde_json::Value::Null;
            assert!(serde_json::from_value::<Settings>(settings.clone()).is_ok());
            settings[field] = original;
        }

        let mut state: serde_json::Value =
            serde_json::from_str(include_str!("../specs/fixtures/state-receiving.json")).unwrap();
        for field in ["mode", "error"] {
            let original = state[field].clone();
            state.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<State>(state.clone()).is_err());
            state[field] = serde_json::Value::Null;
            assert!(serde_json::from_value::<State>(state.clone()).is_ok());
            state[field] = original;
        }
    }
}
