//! Task 09 development-only controller. Task 11 replaces this with worker ownership.
use crate::contracts::{ApiError, ErrorCode, Settings, State, Status};

pub struct SessionController {
    pub(crate) state: State,
}

impl Default for SessionController {
    fn default() -> Self {
        Self {
            state: State {
                status: Status::Stopped,
                mode: None,
                input_peak: 0.0,
                output_peak: 0.0,
                packets_lost: 0,
                buffer_ms: 0,
                error: None,
            },
        }
    }
}

impl SessionController {
    pub fn snapshot(&self) -> State {
        self.state.clone()
    }

    pub fn start(&mut self, _settings: &Settings) -> Result<State, ApiError> {
        self.require_stopped()?;
        Err(ApiError {
            code: ErrorCode::BackendUnavailable,
            message: "Development API only: audio session integration is not available yet".into(),
        })
    }

    pub fn stop(&mut self) -> Result<State, ApiError> {
        let packets_lost = self.state.packets_lost;
        *self = Self::default();
        self.state.packets_lost = packets_lost;
        Ok(self.snapshot())
    }

    pub(crate) fn require_stopped(&self) -> Result<(), ApiError> {
        if self.state.status != Status::Stopped {
            return Err(ApiError {
                code: ErrorCode::LifecycleConflict,
                message: "Stop the session before starting again or changing settings".into(),
            });
        }
        Ok(())
    }
}
