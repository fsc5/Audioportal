//! Loopback HTTP boundary. Enabled explicitly with the development controller.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::contracts::{ApiError, Bootstrap, Devices, ErrorCode, ErrorResponse};
use crate::{session::SessionController, settings};

pub const ADDRESS: &str = "127.0.0.1:46001";
const HOSTS: &[&str] = &["127.0.0.1:46001", "localhost:46001"];
const DEV_HOSTS: &[&str] = &["127.0.0.1:5173", "localhost:5173"];

#[derive(Clone)]
struct Context {
    session: Arc<Mutex<SessionController>>,
    path: PathBuf,
    token: String,
    development_proxy: bool,
}

fn error(code: ErrorCode, message: impl Into<String>) -> ApiError {
    ApiError {
        code,
        message: message.into(),
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.code {
            ErrorCode::InvalidRequest | ErrorCode::InvalidSettings => StatusCode::BAD_REQUEST,
            ErrorCode::LifecycleConflict => StatusCode::CONFLICT,
            ErrorCode::DeviceUnavailable | ErrorCode::BackendUnavailable => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            ErrorCode::WorkerFailed | ErrorCode::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(ErrorResponse { error: self })).into_response()
    }
}

pub async fn bind() -> Result<tokio::net::TcpListener, ApiError> {
    tokio::net::TcpListener::bind(ADDRESS).await.map_err(|err| {
        error(ErrorCode::InternalError, format!("Cannot bind HTTP API to {ADDRESS}: {err}. Close the other instance or free this port."))
    })
}

pub fn router(path: PathBuf, development_proxy: bool) -> Result<Router, ApiError> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| {
        error(
            ErrorCode::InternalError,
            "Cannot generate API bootstrap token",
        )
    })?;
    let token = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(routes(Context {
        session: Arc::new(Mutex::new(SessionController::default())),
        path,
        token,
        development_proxy,
    }))
}

fn routes(context: Context) -> Router {
    Router::new()
        .route("/api/bootstrap", get(bootstrap))
        .route("/api/devices", get(devices))
        .route("/api/state", get(snapshot))
        .route("/api/settings", get(load).put(save))
        .route("/api/start", post(start))
        .route("/api/stop", post(stop))
        .fallback(|| async { error(ErrorCode::InvalidRequest, "Unknown API route") })
        .method_not_allowed_fallback(|| async {
            error(ErrorCode::InvalidRequest, "Unsupported API method")
        })
        .layer(middleware::from_fn_with_state(context.clone(), boundary))
        .with_state(context)
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some(value)
}

async fn boundary(State(context): State<Context>, request: Request, next: Next) -> Response {
    let result = async {
        let headers = request.headers();
        let host = single_header(headers, "host").ok_or_else(|| {
            error(
                ErrorCode::InvalidRequest,
                "A single allowed Host is required",
            )
        })?;
        if !HOSTS.contains(&host) && !(context.development_proxy && DEV_HOSTS.contains(&host)) {
            return Err(error(ErrorCode::InvalidRequest, "Host is not allowed"));
        }
        let mutation = !matches!(
            *request.method(),
            Method::GET | Method::HEAD | Method::OPTIONS
        );
        if mutation || headers.contains_key(header::ORIGIN) {
            let expected = format!("http://{host}");
            if single_header(headers, "origin") != Some(expected.as_str()) {
                return Err(error(
                    ErrorCode::InvalidRequest,
                    "Expected same-origin Origin header",
                ));
            }
        }
        if mutation && single_header(headers, "x-audioportal-token") != Some(context.token.as_str())
        {
            return Err(error(
                ErrorCode::InvalidRequest,
                "Missing or incorrect API token",
            ));
        }
        let is_settings =
            request.method() == Method::PUT && request.uri().path() == "/api/settings";
        if is_settings
            && !single_header(headers, "content-type").is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
            })
        {
            return Err(error(
                ErrorCode::InvalidRequest,
                "Settings require application/json",
            ));
        }
        let (parts, body) = request.into_parts();
        let bytes = to_bytes(body, settings::MAX_SETTINGS_BYTES)
            .await
            .map_err(|_| {
                error(
                    ErrorCode::InvalidRequest,
                    "Unreadable body or body exceeds 16 KiB",
                )
            })?;
        if !is_settings && !bytes.is_empty() {
            return Err(error(
                ErrorCode::InvalidRequest,
                "This route requires an empty body",
            ));
        }
        Ok(next
            .run(Request::from_parts(parts, Body::from(bytes)))
            .await)
    }
    .await;
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static header"),
    );
    response
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<Json<T>, ApiError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| {
            error(
                ErrorCode::InternalError,
                "API operation failed unexpectedly",
            )
        })?
        .map(Json)
}

async fn bootstrap(State(context): State<Context>) -> Json<Bootstrap> {
    Json(Bootstrap {
        csrf_token: context.token,
    })
}

async fn snapshot(
    State(context): State<Context>,
) -> Result<Json<crate::contracts::State>, ApiError> {
    blocking(move || Ok(context.session.lock().map_err(|_| poisoned())?.snapshot())).await
}

fn poisoned() -> ApiError {
    error(
        ErrorCode::InternalError,
        "Session controller failed; restart Audioportal",
    )
}

async fn load(
    State(context): State<Context>,
) -> Result<Json<crate::contracts::Settings>, ApiError> {
    blocking(move || settings::load(&context.path)).await
}

async fn save(
    State(context): State<Context>,
    body: axum::body::Bytes,
) -> Result<Json<crate::contracts::Settings>, ApiError> {
    blocking(move || {
        let settings = settings::parse(&body)?;
        let session = context.session.lock().map_err(|_| poisoned())?;
        session.require_stopped()?;
        settings::save(&context.path, &settings)?;
        Ok(settings)
    })
    .await
}

async fn start(State(context): State<Context>) -> Result<Json<crate::contracts::State>, ApiError> {
    blocking(move || {
        let mut session = context.session.lock().map_err(|_| poisoned())?;
        session.require_stopped()?;
        session.start(&settings::load(&context.path)?)
    })
    .await
}

async fn stop(State(context): State<Context>) -> Result<Json<crate::contracts::State>, ApiError> {
    blocking(move || context.session.lock().map_err(|_| poisoned())?.stop()).await
}

async fn devices() -> Result<Json<Devices>, ApiError> {
    blocking(enumerate_devices).await
}

#[cfg(not(windows))]
fn enumerate_devices() -> Result<Devices, ApiError> {
    Err(error(
        ErrorCode::BackendUnavailable,
        "Audio device enumeration requires Windows",
    ))
}

#[cfg(windows)]
fn enumerate_devices() -> Result<Devices, ApiError> {
    use crate::contracts::{Device, DeviceKind};
    wasapi::initialize_mta().ok().map_err(|err| {
        error(
            ErrorCode::BackendUnavailable,
            format!("Cannot initialize Windows audio: {err}"),
        )
    })?;
    let result = crate::audio::windows::list_devices()
        .map(|endpoints| Devices {
            devices: endpoints
                .into_iter()
                .map(|endpoint| Device {
                    id: endpoint.id,
                    name: endpoint.name,
                    kind: if endpoint.kind == "capture" {
                        DeviceKind::Capture
                    } else {
                        DeviceKind::Playback
                    },
                })
                .collect(),
        })
        .map_err(|err| {
            error(
                ErrorCode::DeviceUnavailable,
                format!("Cannot enumerate audio endpoints: {err}"),
            )
        });
    wasapi::deinitialize();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{Settings, Status};
    use serde_json::{json, Value};
    use tower::ServiceExt;

    fn test_context(path: PathBuf, development_proxy: bool) -> Context {
        Context {
            session: Arc::new(Mutex::new(SessionController::default())),
            path,
            token: "test-token".into(),
            development_proxy,
        }
    }

    fn request(method: &str, path: &str, body: impl Into<Body>) -> Request {
        Request::builder()
            .method(method)
            .uri(path)
            .header("host", ADDRESS)
            .header("origin", format!("http://{ADDRESS}"))
            .header("x-audioportal-token", "test-token")
            .header("content-type", "application/json")
            .body(body.into())
            .unwrap()
    }

    async fn send(app: &Router, request: Request) -> (StatusCode, Value) {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(!response
            .headers()
            .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn rejected(app: &Router, request: Request, status: StatusCode, code: ErrorCode) {
        let (actual, body) = send(app, request).await;
        assert_eq!(actual, status, "{body}");
        let body: ErrorResponse = serde_json::from_value(body).unwrap();
        assert_eq!(body.error.code, code);
        assert!(!body.error.message.is_empty());
    }

    #[tokio::test]
    async fn settings_lifecycle_and_error_routes() {
        let dir = tempfile::tempdir().unwrap();
        let context = test_context(dir.path().join("settings.json"), false);
        let app = routes(context.clone());
        let (_, defaults) = send(&app, request("GET", "/api/settings", "")).await;
        assert_eq!(defaults, serde_json::to_value(Settings::default()).unwrap());
        let (_, stopped) = send(&app, request("GET", "/api/state", "")).await;
        assert_eq!(
            stopped,
            serde_json::from_str::<Value>(include_str!("../specs/fixtures/state-stopped.json"))
                .unwrap()
        );
        let saved = include_str!("../specs/fixtures/settings.json");
        let (status, value) = send(&app, request("PUT", "/api/settings", saved)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value, serde_json::from_str::<Value>(saved).unwrap());
        assert_eq!(
            send(
                &routes(context.clone()),
                request("GET", "/api/settings", "")
            )
            .await
            .1,
            value
        );
        assert_eq!(
            serde_json::to_value(settings::load(&context.path).unwrap()).unwrap(),
            value
        );
        rejected(
            &app,
            request("POST", "/api/start", ""),
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::BackendUnavailable,
        )
        .await;
        assert_eq!(
            send(&app, request("GET", "/api/state", "")).await.1,
            stopped
        );
        for status in [
            Status::Starting,
            Status::Listening,
            Status::Receiving,
            Status::Sending,
            Status::Stopping,
            Status::Failed,
        ] {
            context.session.lock().unwrap().state.status = status;
            for (method, path, body) in
                [("PUT", "/api/settings", saved), ("POST", "/api/start", "")]
            {
                rejected(
                    &app,
                    request(method, path, body),
                    StatusCode::CONFLICT,
                    ErrorCode::LifecycleConflict,
                )
                .await;
            }
        }
        for _ in 0..2 {
            assert_eq!(
                send(&app, request("POST", "/api/stop", "")).await,
                (StatusCode::OK, stopped.clone())
            );
        }
        for (method, path) in [("GET", "/api/missing"), ("DELETE", "/api/state")] {
            rejected(
                &app,
                request(method, path, ""),
                StatusCode::BAD_REQUEST,
                ErrorCode::InvalidRequest,
            )
            .await;
        }
        #[cfg(not(windows))]
        rejected(
            &app,
            request("GET", "/api/devices", ""),
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::BackendUnavailable,
        )
        .await;
        #[cfg(windows)]
        {
            let (status, value) = send(&app, request("GET", "/api/devices", "")).await;
            assert_eq!(status, StatusCode::OK, "{value}");
            let _: Devices = serde_json::from_value(value).unwrap();
        }
    }

    #[tokio::test]
    async fn invalid_input_and_storage_errors_preserve_settings() {
        let dir = tempfile::tempdir().unwrap();
        let context = test_context(dir.path().join("settings.json"), false);
        settings::save(&context.path, &Settings::default()).unwrap();
        let original = std::fs::read(&context.path).unwrap();
        let app = routes(context.clone());
        let mut unknown = serde_json::to_value(Settings::default()).unwrap();
        unknown["unknown"] = json!(1);
        for body in [
            "{broken".into(),
            "{}".into(),
            unknown.to_string(),
            " ".repeat(settings::MAX_SETTINGS_BYTES + 1),
        ] {
            rejected(
                &app,
                request("PUT", "/api/settings", body),
                StatusCode::BAD_REQUEST,
                ErrorCode::InvalidRequest,
            )
            .await;
            assert_eq!(std::fs::read(&context.path).unwrap(), original);
        }
        let mut invalid = serde_json::to_value(Settings::default()).unwrap();
        invalid["buffer_ms"] = json!(31);
        rejected(
            &app,
            request("PUT", "/api/settings", invalid.to_string()),
            StatusCode::BAD_REQUEST,
            ErrorCode::InvalidSettings,
        )
        .await;
        for path in ["/api/start", "/api/stop"] {
            rejected(
                &app,
                request("POST", path, "{}"),
                StatusCode::BAD_REQUEST,
                ErrorCode::InvalidRequest,
            )
            .await;
        }
        let mut wrong_type = request("PUT", "/api/settings", original.clone());
        wrong_type
            .headers_mut()
            .insert(header::CONTENT_TYPE, "text/plain".parse().unwrap());
        rejected(
            &app,
            wrong_type,
            StatusCode::BAD_REQUEST,
            ErrorCode::InvalidRequest,
        )
        .await;
        assert_eq!(std::fs::read(&context.path).unwrap(), original);
        assert_eq!(
            context.session.lock().unwrap().snapshot().status,
            Status::Stopped
        );
        std::fs::write(&context.path, b"{corrupt").unwrap();
        for (method, path) in [("GET", "/api/settings"), ("POST", "/api/start")] {
            rejected(
                &app,
                request(method, path, ""),
                StatusCode::BAD_REQUEST,
                ErrorCode::InvalidRequest,
            )
            .await;
        }
        assert_eq!(std::fs::read(&context.path).unwrap(), b"{corrupt");
        assert_eq!(
            send(&app, request("PUT", "/api/settings", original.clone()))
                .await
                .0,
            StatusCode::OK
        );
        // A file as parent deterministically forces a persistence error on all platforms.
        let blocked = routes(test_context(dir.path().join("settings.json/child"), false));
        rejected(
            &blocked,
            request("PUT", "/api/settings", original.clone()),
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::InternalError,
        )
        .await;
        assert_eq!(std::fs::read(&context.path).unwrap(), original);
    }

    #[tokio::test]
    async fn security_boundary_and_development_proxy_policy() {
        let dir = tempfile::tempdir().unwrap();
        for development_proxy in [false, true] {
            let app = routes(test_context(
                dir.path().join("settings.json"),
                development_proxy,
            ));
            for (header, values) in [
                (
                    "host",
                    vec![
                        None,
                        Some("evil.example"),
                        Some("127.0.0.1:46002"),
                        Some("localhost:46001.evil.example"),
                    ],
                ),
                (
                    "origin",
                    vec![
                        None,
                        Some("null"),
                        Some("http://evil.example"),
                        Some("https://127.0.0.1:46001"),
                    ],
                ),
                ("x-audioportal-token", vec![None, Some("wrong")]),
            ] {
                for value in values {
                    let mut req = request("POST", "/api/stop", "");
                    req.headers_mut().remove(header);
                    if let Some(value) = value {
                        req.headers_mut().insert(header, value.parse().unwrap());
                    }
                    rejected(
                        &app,
                        req,
                        StatusCode::BAD_REQUEST,
                        ErrorCode::InvalidRequest,
                    )
                    .await;
                }
                let mut duplicate = request("POST", "/api/stop", "");
                let value = duplicate.headers()[header].clone();
                duplicate.headers_mut().append(header, value);
                rejected(
                    &app,
                    duplicate,
                    StatusCode::BAD_REQUEST,
                    ErrorCode::InvalidRequest,
                )
                .await;
            }
            for host in HOSTS.iter().chain(DEV_HOSTS.iter()) {
                let mut req = request("POST", "/api/stop", "");
                req.headers_mut()
                    .insert(header::HOST, host.parse().unwrap());
                req.headers_mut()
                    .insert(header::ORIGIN, format!("http://{host}").parse().unwrap());
                let expected = if HOSTS.contains(host) || development_proxy {
                    StatusCode::OK
                } else {
                    StatusCode::BAD_REQUEST
                };
                assert_eq!(send(&app, req).await.0, expected);
            }
            for header in ["host", "origin"] {
                let mut req = request("GET", "/api/bootstrap", "");
                req.headers_mut()
                    .insert(header, "http://evil.example".parse().unwrap());
                rejected(
                    &app,
                    req,
                    StatusCode::BAD_REQUEST,
                    ErrorCode::InvalidRequest,
                )
                .await;
            }
            assert!(!dir.path().join("settings.json").exists());
        }
    }

    #[tokio::test]
    async fn bootstrap_token_is_fresh_and_authorizes_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let mut previous = String::new();
        for _ in 0..2 {
            let app = router(dir.path().join("settings.json"), false).unwrap();
            let mut req = request("GET", "/api/bootstrap", "");
            req.headers_mut().remove("origin");
            req.headers_mut().remove("x-audioportal-token");
            let (status, value) = send(&app, req).await;
            assert_eq!(status, StatusCode::OK);
            let bootstrap: Bootstrap = serde_json::from_value(value).unwrap();
            assert_eq!(bootstrap.csrf_token.len(), 64);
            assert_ne!(bootstrap.csrf_token, previous);
            previous = bootstrap.csrf_token;
            let mut req = request("POST", "/api/stop", "");
            req.headers_mut()
                .insert("x-audioportal-token", previous.parse().unwrap());
            assert_eq!(send(&app, req).await.0, StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn fixed_loopback_listener_reports_conflict() {
        let listener = bind().await.unwrap();
        assert_eq!(listener.local_addr().unwrap(), ADDRESS.parse().unwrap());
        let error = bind().await.unwrap_err();
        assert_eq!(error.code, ErrorCode::InternalError);
        assert!(error.message.contains(ADDRESS));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn waiting_on_controller_does_not_block_executor() {
        let dir = tempfile::tempdir().unwrap();
        let context = test_context(dir.path().join("settings.json"), false);
        let app = routes(context.clone());
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _guard = context.session.lock().unwrap();
            ready_tx.send(()).unwrap();
            // A bounded wait also lets an incorrectly blocking test fail rather than hang.
            let _ = release_rx.recv_timeout(std::time::Duration::from_secs(2));
        });
        ready_rx.recv().unwrap();
        let waiting = tokio::spawn(app.clone().oneshot(request("POST", "/api/stop", "")));
        let start = std::time::Instant::now();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(
            send(&app, request("GET", "/api/bootstrap", "")).await.0,
            StatusCode::OK
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        assert!(!waiting.is_finished());
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        assert_eq!(waiting.await.unwrap().unwrap().status(), StatusCode::OK);
    }

    #[test]
    fn all_error_codes_have_contract_statuses() {
        for (code, status) in [
            (ErrorCode::InvalidRequest, 400),
            (ErrorCode::InvalidSettings, 400),
            (ErrorCode::LifecycleConflict, 409),
            (ErrorCode::DeviceUnavailable, 503),
            (ErrorCode::BackendUnavailable, 503),
            (ErrorCode::WorkerFailed, 500),
            (ErrorCode::InternalError, 500),
        ] {
            assert_eq!(
                error(code, "test").into_response().status().as_u16(),
                status
            );
        }
    }
}
