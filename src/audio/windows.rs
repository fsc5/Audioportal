use std::error::Error;
use wasapi::{Device, DeviceEnumerator, DeviceState, Direction, ShareMode, WaveFormat};

pub mod capture;
pub use capture::start_capture;

pub type AudioResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug)]
pub struct Endpoint {
    pub id: String,
    pub name: String,
    pub kind: &'static str,
    pub mix_format: WaveFormat,
    pub device_format: WaveFormat,
    pub default_period_hns: i64,
    pub min_period_hns: i64,
    pub float_48k_stereo: String,
}

// Call on a thread where COM has already been initialized (initialize_mta).
pub fn list_devices() -> AudioResult<Vec<Endpoint>> {
    let enumerator = DeviceEnumerator::new()?;
    let mut endpoints = Vec::new();
    for (direction, kind) in [
        (Direction::Capture, "capture"),
        (Direction::Render, "playback"),
    ] {
        for device in &enumerator.get_device_collection(&direction)? {
            let device = device?;
            let client = device.get_iaudioclient()?;
            let (default_period_hns, min_period_hns) = client.get_device_period()?;
            let float_format = WaveFormat::new(32, 32, &wasapi::SampleType::Float, 48_000, 2, None);
            let float_48k_stereo = match client.is_supported(&float_format, &ShareMode::Shared) {
                Ok(None) => "supported in shared mode".to_string(),
                Ok(Some(closest)) => format!("conversion needed; closest: {closest:?}"),
                Err(err) => format!("query failed ({err}); try shared-mode autoconvert"),
            };
            endpoints.push(Endpoint {
                id: device.get_id()?,
                name: device.get_friendlyname()?,
                kind,
                mix_format: client.get_mixformat()?,
                device_format: device.get_device_format()?,
                default_period_hns,
                min_period_hns,
                float_48k_stereo,
            });
        }
    }
    Ok(endpoints)
}

// Select by the opaque endpoint ID, never by a display name or the current default.
pub fn selected_device(id: &str, direction: Direction) -> AudioResult<Device> {
    let enumerator = DeviceEnumerator::new()?;
    let device = enumerator.get_device(id).map_err(|err| {
        format!("endpoint {id:?} is unavailable ({err}); run `audio_probe list` again")
    })?;
    if device.get_direction() != direction {
        return Err(format!("endpoint {id:?} is not a {direction:?} endpoint").into());
    }
    let state = device.get_state().map_err(|err| {
        format!("cannot check endpoint {id:?} availability ({err}); run `audio_probe list` again")
    })?;
    if state != DeviceState::Active {
        return Err(format!(
            "endpoint {id:?} is {state}; connect or enable it and run `audio_probe list` again"
        )
        .into());
    }
    Ok(device)
}
