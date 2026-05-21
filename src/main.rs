use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{sse::{Event, KeepAlive, Sse}, IntoResponse},
    routing::get,
    Json, Router,
};
use futures::stream::Stream;
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc},
    time,
};
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use tracing::{info, warn};
use walkdir::WalkDir;

#[derive(Clone)]
struct AppState {
    tx: broadcast::Sender<String>,
    active_clients: Arc<AtomicUsize>,
    config: Arc<Config>,
}

#[derive(Debug)]
struct Config {
    watch_dir: PathBuf,
    host: String,
    port: u16,
    extensions: HashSet<String>,
    poll_interval: Duration,
    max_clients: usize,
    client_queue_size: usize,
    sse_heartbeat: Duration,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let watch_dir = PathBuf::from(env::var("WATCH_DIR").unwrap_or_else(|_| "/site".into()));
        let watch_dir = watch_dir.canonicalize().unwrap_or(watch_dir);
        let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
        let port = env::var("PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(8765);
        let extensions: HashSet<String> = env::var("EXTENSIONS")
            .unwrap_or_else(|_| ".html,.css,.js".into())
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        let poll_interval = Duration::from_secs_f64(
            env::var("POLL_INTERVAL")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.5)
                .max(0.2),
        );
        let max_clients = env::var("MAX_CLIENTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100)
            .max(1);
        let client_queue_size = env::var("CLIENT_QUEUE_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8)
            .max(1);
        let sse_heartbeat = Duration::from_secs_f64(
            env::var("SSE_HEARTBEAT")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(15.0)
                .max(5.0),
        );

        if extensions.is_empty() {
            return Err("EXTENSIONS must not be empty".into());
        }
        if watch_dir == Path::new("/") {
            return Err("WATCH_DIR=/ is not allowed".into());
        }
        if !watch_dir.is_absolute() {
            return Err("WATCH_DIR must resolve to an absolute path".into());
        }

        Ok(Self {
            watch_dir,
            host,
            port,
            extensions,
            poll_interval,
            max_clients,
            client_queue_size,
            sse_heartbeat,
        })
    }
}

fn scan_files(config: &Config) -> HashMap<String, u128> {
    let mut found = HashMap::new();
    if !config.watch_dir.exists() {
        return found;
    }

    for entry in WalkDir::new(&config.watch_dir).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| format!(".{}", s.to_ascii_lowercase()));
        if ext.as_ref().is_none_or(|e| !config.extensions.contains(e)) {
            continue;
        }
        if let Ok(meta) = path.metadata() {
            if let Ok(modified) = meta.modified() {
                if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
                    found.insert(path.display().to_string(), duration.as_nanos());
                }
            }
        }
    }
    found
}

async fn watch_loop(state: AppState) {
    let mut previous = scan_files(&state.config);
    info!(
        watch_dir = %state.config.watch_dir.display(),
        extensions = %state.config.extensions.iter().cloned().collect::<Vec<_>>().join(","),
        poll_interval = ?state.config.poll_interval,
        initial_files = previous.len(),
        "watch loop started"
    );

    loop {
        time::sleep(state.config.poll_interval).await;
        let started = Instant::now();
        let new_state = scan_files(&state.config);

        if new_state != previous {
            let changed_count = new_state
                .iter()
                .filter(|(p, v)| previous.get(*p) != Some(*v))
                .count();
            let removed_count = previous.keys().filter(|p| !new_state.contains_key(*p)).count();
            info!(
                changed = changed_count,
                removed = removed_count,
                active_clients = state.active_clients.load(Ordering::Relaxed),
                "filesystem change detected"
            );
            let _ = state.tx.send(r#"{"type":"reload"}"#.to_string());
            previous = new_state;
        }

        let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
        if duration_ms > 1000.0 {
            warn!(duration_ms, files = previous.len(), "slow scan");
        }
    }
}

async fn health() -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert("Cache-Control", HeaderValue::from_static("no-store"));
    headers.insert("X-Content-Type-Options", HeaderValue::from_static("nosniff"));
    (headers, Json(json!({"ok": true})))
}

async fn sse(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    let ip = addr.ip().to_string();
    let active = state.active_clients.load(Ordering::SeqCst);
    if active >= state.config.max_clients {
        warn!(ip = %ip, active, "rejecting client limit reached");
        let mut headers = HeaderMap::new();
        headers.insert("Retry-After", HeaderValue::from_static("5"));
        return Err((StatusCode::SERVICE_UNAVAILABLE, headers, "too many clients"));
    }

    let mut rx = state.tx.subscribe();
    let (client_tx, client_rx) = mpsc::channel::<Result<Event, std::convert::Infallible>>(state.config.client_queue_size);
    state.active_clients.fetch_add(1, Ordering::SeqCst);
    info!(ip = %ip, active = state.active_clients.load(Ordering::SeqCst), "client connected");

    let heartbeat = state.config.sse_heartbeat;
    let active_clients = state.active_clients.clone();
    tokio::spawn(async move {
        let _ = client_tx.send(Ok(Event::default().comment("connected").retry(Duration::from_millis(1000)))).await;
        loop {
            match time::timeout(heartbeat, rx.recv()).await {
                Ok(Ok(msg)) => {
                    if client_tx.send(Ok(Event::default().data(msg))).await.is_err() {
                        break;
                    }
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped))) => {
                    warn!(ip = %ip, skipped, "client lagged behind broadcast stream");
                    break;
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => break,
                Err(_) => {
                    if client_tx.send(Ok(Event::default().comment("heartbeat"))).await.is_err() {
                        break;
                    }
                }
            }
        }
        let remaining = active_clients.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
        info!(ip = %ip, active = remaining, "client disconnected");
    });

    let mut headers = HeaderMap::new();
    headers.insert("Cache-Control", HeaderValue::from_static("no-cache, no-store, must-revalidate"));
    headers.insert("Connection", HeaderValue::from_static("keep-alive"));
    headers.insert("X-Accel-Buffering", HeaderValue::from_static("no"));
    headers.insert("Access-Control-Allow-Origin", HeaderValue::from_static("*"));
    headers.insert("X-Content-Type-Options", HeaderValue::from_static("nosniff"));

    let stream = ReceiverStream::new(client_rx);
    Ok((headers, Sse::new(stream).keep_alive(KeepAlive::default())))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(true)
        .init();

    let config = Arc::new(Config::from_env().map_err(|e| format!("config error: {e}"))?);
    info!(
        host = %config.host,
        port = config.port,
        watch_dir = %config.watch_dir.display(),
        max_clients = config.max_clients,
        queue_size = config.client_queue_size,
        "starting server"
    );

    let (tx, _) = broadcast::channel::<String>(config.client_queue_size.max(16) * 8);
    let state = AppState {
        tx,
        active_clients: Arc::new(AtomicUsize::new(0)),
        config: config.clone(),
    };

    tokio::spawn(watch_loop(state.clone()));

    let app = Router::new()
        .route("/events", get(sse))
        .route("/healthz", get(health))
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            info!("shutdown complete");
        })
        .await?;

    Ok(())
}
