//! Agent access to Annotate's captures: an MCP server on `127.0.0.1:PORT` that coding agents
//! call for a capture's summary, contact sheet, keyframes and manifest. An agent on another
//! machine reaches it through an SSH `RemoteForward`, or through this Mac's Tailscale address
//! when the reach is `McpReach::Tailnet`. A capture leaves this Mac only when an agent asks for it.
//! Every request needs the bearer token kept in `annotate/mcp-token`.
//!
//! The server speaks MCP's Streamable HTTP transport without sessions or server-sent events:
//! each POST carries one JSON-RPC message and gets one JSON reply.

mod tools;

use std::convert::Infallible;
use std::ffi::CStr;
use std::fs;
use std::future::Future;
use std::io::Write as _;
use std::net::Ipv4Addr;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result};
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};

use crate::app_handle::AppHandle;
use crate::settings::{get_settings, write_settings, McpReach};

/// The port on this Mac, and the one the `RemoteForward` line opens on the remote machine.
pub const PORT: u16 = 47790;
/// MCP revisions this server answers to, newest first. It uses nothing that changed between
/// them.
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const INSTRUCTIONS: &str = include_str!("mcp/instructions.md");
/// The largest request body read. Requests are small JSON-RPC messages.
const MAX_BODY: usize = 1 << 20;
/// How often the server checks whether this Mac's Tailscale address has changed.
const TAILSCALE_POLL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Off,
    Listening,
    /// The server didn't start, usually because another app holds the port.
    Failed(String),
}

static STATUS: parking_lot::Mutex<Status> = parking_lot::Mutex::new(Status::Off);

/// The running server's accept loop. Locked across a restart so two syncs can't race for the
/// port.
fn server() -> &'static tokio::sync::Mutex<Option<JoinHandle<()>>> {
    static SERVER: OnceLock<tokio::sync::Mutex<Option<JoinHandle<()>>>> = OnceLock::new();
    SERVER.get_or_init(Default::default)
}

pub fn status() -> Status {
    STATUS.lock().clone()
}

fn url(host: impl std::fmt::Display) -> String {
    format!("http://{host}:{PORT}/mcp")
}

/// The URL agents add: this Mac's Tailscale address while the server listens there, otherwise
/// loopback, which `ssh_forward` carries to remote machines.
fn agent_url(app: &AppHandle) -> Result<String> {
    if get_settings(app).annotate_mcp_reach != McpReach::Tailnet {
        return Ok(url(Ipv4Addr::LOCALHOST));
    }
    let address = tailscale_address().context("Tailscale isn't connected on this Mac")?;
    Ok(url(address))
}

/// This Mac's Tailscale address: an IPv4 address in Tailscale's 100.64.0.0/10 range on its
/// tunnel interface (`utun*` on macOS, `tailscale0` on Linux). `None` while Tailscale is off.
pub fn tailscale_address() -> Option<Ipv4Addr> {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: on success `list` holds a linked list that `freeifaddrs` releases below.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return None;
    }
    let mut found = None;
    let mut cursor = list;
    while !cursor.is_null() {
        // SAFETY: every node, its name and its address stay valid until `freeifaddrs`.
        let entry = unsafe { &*cursor };
        cursor = entry.ifa_next;
        if entry.ifa_addr.is_null()
            || i32::from(unsafe { (*entry.ifa_addr).sa_family }) != libc::AF_INET
        {
            continue;
        }
        let name = unsafe { CStr::from_ptr(entry.ifa_name) }.to_bytes();
        if !(name.starts_with(b"utun") || name == b"tailscale0") {
            continue;
        }
        // SAFETY: an `AF_INET` address is a `sockaddr_in`.
        let address = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in>() };
        let address = Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr));
        if is_tailscale(address) {
            found = Some(address);
            break;
        }
    }
    // SAFETY: `list` came from `getifaddrs` and nothing borrows from it any more.
    unsafe { libc::freeifaddrs(list) };
    found
}

/// Tailscale gives each machine an address in the CGNAT range, 100.64.0.0/10.
fn is_tailscale(address: Ipv4Addr) -> bool {
    let [first, second, ..] = address.octets();
    first == 100 && (64..128).contains(&second)
}

/// The `~/.ssh/config` line that opens `PORT` on a remote machine and forwards it here.
pub fn ssh_forward() -> String {
    format!("RemoteForward {PORT} 127.0.0.1:{PORT}")
}

/// Adds the server to Claude Code for every project on the machine it runs on.
pub fn claude_command(app: &AppHandle) -> Result<String> {
    Ok(format!(
        "claude mcp add --transport http --scope user bespoke {} --header \"Authorization: Bearer {}\"",
        agent_url(app)?,
        token(app)?
    ))
}

/// The `~/.codex/config.toml` entry that adds the server to Codex. `codex mcp add` can't store
/// a header, so Codex gets a config entry instead of a command.
pub fn codex_config(app: &AppHandle) -> Result<String> {
    Ok(format!(
        "[mcp_servers.bespoke]\nurl = \"{}\"\nhttp_headers = {{ Authorization = \"Bearer {}\" }}\n",
        agent_url(app)?,
        token(app)?
    ))
}

/// Saves where the server listens and starts, stops or restarts it to match.
pub fn set_reach(app: AppHandle, reach: McpReach) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.annotate_mcp_reach = reach;
    write_settings(&app, settings);
    sync(&app);
    Ok(())
}

/// Replaces the token and restarts the server with it. Agents need the new command.
pub fn renew_token(app: &AppHandle) -> Result<()> {
    let path = token_path(app)?;
    match fs::remove_file(&path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(error).with_context(|| format!("Could not remove {}", path.display()));
        }
        _ => {}
    }
    token(app)?;
    sync(app);
    Ok(())
}

/// Starts, stops or restarts the server to match the setting and the token. Returns at once;
/// `status` reports how it went.
pub fn sync(app: &AppHandle) {
    let app = app.clone();
    crate::runtime::spawn(async move {
        let mut running = server().lock().await;
        if let Some(task) = running.take() {
            task.abort();
            // The listener closes when its task is dropped, which frees the port to bind again.
            let _ = task.await;
        }
        if get_settings(&app).annotate_mcp_reach == McpReach::Off {
            *STATUS.lock() = Status::Off;
            return;
        }
        match listen(&app).await {
            Ok(serving) => {
                *running = Some(crate::runtime::spawn(serving));
                log::info!(
                    "Annotate MCP server listening on {}",
                    url(Ipv4Addr::LOCALHOST)
                );
                *STATUS.lock() = Status::Listening;
            }
            Err(error) => {
                log::warn!("Annotate MCP server didn't start: {error:#}");
                *STATUS.lock() = Status::Failed(format!("{error:#}"));
            }
        }
    });
}

async fn listen(app: &AppHandle) -> Result<impl Future<Output = ()>> {
    let service = Arc::new(Service {
        token: token(app)?,
        captures: scrawl::host::captures_dir(),
    });
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, PORT))
        .await
        .with_context(|| format!("Couldn't listen on 127.0.0.1:{PORT}"))?;
    let tailscale = get_settings(app).annotate_mcp_reach == McpReach::Tailnet;
    Ok(async move {
        if tailscale {
            tokio::join!(serve(listener, service.clone()), serve_tailscale(service));
        } else {
            serve(listener, service).await;
        }
    })
}

/// Serves on this Mac's Tailscale address and follows it as Tailscale connects, disconnects or
/// changes address. Runs until the task is aborted, which drops the `JoinSet` and closes the
/// listener with it.
async fn serve_tailscale(service: Arc<Service>) {
    let mut current = None;
    let mut serving = JoinSet::new();
    loop {
        let address = tailscale_address();
        if address != current {
            serving.abort_all();
            while serving.join_next().await.is_some() {}
            current = address;
            if let Some(address) = address {
                match TcpListener::bind((address, PORT)).await {
                    Ok(listener) => {
                        log::info!("Annotate MCP server listening on {}", url(address));
                        serving.spawn(serve(listener, service.clone()));
                    }
                    // Tried again when the address changes or the switch restarts the server.
                    Err(error) => {
                        log::warn!("Annotate MCP server couldn't listen on {address}: {error}")
                    }
                }
            }
        }
        tokio::time::sleep(TAILSCALE_POLL).await;
    }
}

/// Accepts connections until the task is aborted. Dropping the `JoinSet` with it closes every
/// open connection too.
async fn serve(listener: TcpListener, service: Arc<Service>) {
    let mut connections = JoinSet::new();
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(error) => {
                log::warn!("Annotate MCP server accept failed: {error}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        let service = service.clone();
        connections.spawn(async move {
            let handler = hyper::service::service_fn(move |request| {
                let service = service.clone();
                async move { Ok::<_, Infallible>(service.handle(request).await) }
            });
            if let Err(error) = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), handler)
                .await
            {
                log::debug!("Annotate MCP connection ended: {error}");
            }
        });
        while connections.try_join_next().is_some() {}
    }
}

fn token_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(crate::annotate::data_dir(app)
        .context("No data directory")?
        .join("mcp-token"))
}

/// The bearer token agents send, made on first use. It has its own file, readable only by this
/// user, so it stays out of the settings and the logs.
fn token(app: &AppHandle) -> Result<String> {
    let path = token_path(app)?;
    if let Ok(token) = fs::read_to_string(&path) {
        let token = token.trim();
        if !token.is_empty() {
            return Ok(token.to_owned());
        }
    }
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("No randomness: {error}"))?;
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    write_private(&path, &token)?;
    Ok(token)
}

fn write_private(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| file.write_all(contents.as_bytes()))
        .with_context(|| format!("Could not write {}", path.display()))
}

struct Service {
    token: String,
    captures: PathBuf,
}

impl Service {
    async fn handle(&self, request: Request<Incoming>) -> Response<Full<Bytes>> {
        if request.uri().path() != "/mcp" {
            return empty(StatusCode::NOT_FOUND);
        }
        // Browsers send `Origin` and agents don't. Refusing other origins keeps a web page from
        // reaching the server through DNS rebinding.
        if request
            .headers()
            .get(header::ORIGIN)
            .is_some_and(|origin| !is_loopback_origin(origin))
        {
            return empty(StatusCode::FORBIDDEN);
        }
        if !self.authorized(request.headers()) {
            let mut response = empty(StatusCode::UNAUTHORIZED);
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            return response;
        }
        if request.method() != Method::POST {
            // No server-sent event stream (GET) and no sessions to end (DELETE).
            let mut response = empty(StatusCode::METHOD_NOT_ALLOWED);
            response
                .headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static("POST"));
            return response;
        }
        let body = match Limited::new(request.into_body(), MAX_BODY).collect().await {
            Ok(body) => body.to_bytes(),
            Err(_) => return empty(StatusCode::PAYLOAD_TOO_LARGE),
        };
        let Ok(message) = serde_json::from_slice::<Value>(&body) else {
            return reply(
                StatusCode::BAD_REQUEST,
                &json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}}),
            );
        };
        match self.rpc(message).await {
            Some(answer) => reply(StatusCode::OK, &answer),
            None => empty(StatusCode::ACCEPTED),
        }
    }

    fn authorized(&self, headers: &HeaderMap) -> bool {
        let Some(presented) = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
        else {
            return false;
        };
        // Compares every byte, so the time taken doesn't tell how much of the token matched.
        presented.len() == self.token.len()
            && presented
                .bytes()
                .zip(self.token.bytes())
                .fold(0, |diff, (a, b)| diff | (a ^ b))
                == 0
    }

    /// Answers one JSON-RPC message. `None` for notifications and replies, which get no answer.
    async fn rpc(&self, message: Value) -> Option<Value> {
        let id = message.get("id")?.clone();
        let method = message.get("method")?.as_str()?.to_owned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let result = match method.as_str() {
            "initialize" => Ok(initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools::list()})),
            "tools/call" => self.call(params).await,
            _ => Err((-32601, format!("Method not found: {method}"))),
        };
        Some(match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        })
    }

    /// A failing tool is a result with `isError`, which the agent reads. Only a tool that doesn't
    /// exist is a protocol error.
    async fn call(&self, params: Value) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let captures = self.captures.clone();
        let tool = name.clone();
        let outcome =
            crate::runtime::spawn_blocking(move || tools::call(&captures, &tool, &arguments))
                .await
                .map_err(|error| (-32603, format!("{name} failed: {error}")))?;
        match outcome {
            Some(Ok(content)) => Ok(json!({"content": content})),
            Some(Err(error)) => {
                log::debug!("Annotate MCP {name}: {error:#}");
                Ok(json!({
                    "content": [{"type": "text", "text": format!("{error:#}")}],
                    "isError": true,
                }))
            }
            None => Err((-32602, format!("Unknown tool: {name}"))),
        }
    }
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|version| PROTOCOL_VERSIONS.contains(version))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "bespoke", "title": "Bespoke Annotate", "version": env!("CARGO_PKG_VERSION")},
        "instructions": INSTRUCTIONS,
    })
}

fn is_loopback_origin(origin: &HeaderValue) -> bool {
    let Some(rest) = origin.to_str().ok().and_then(|origin| {
        origin
            .strip_prefix("http://")
            .or_else(|| origin.strip_prefix("https://"))
    }) else {
        return false;
    };
    let host = match rest.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => rest.split([':', '/']).next().unwrap_or_default(),
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn empty(status: StatusCode) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::new()));
    *response.status_mut() = status;
    response
}

fn reply(status: StatusCode, body: &Value) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(body.to_string())));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use image::GenericImageView as _;

    const TOKEN: &str = "secret";

    /// A captures folder with one capture whose contact sheet is wider than agents get.
    fn captures() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("scrawl-1-a.scrawl");
        fs::create_dir_all(bundle.join("frames")).unwrap();
        image::RgbaImage::new(3200, 100)
            .save(bundle.join("contact-sheet.png"))
            .unwrap();
        image::RgbaImage::new(40, 30)
            .save(bundle.join("frames/frame-01.png"))
            .unwrap();
        let manifest = json!({
            "version": 1, "id": "scrawl-1-a", "createdAtMs": 100, "durationMs": 7_000,
            "target": {"mode": "display", "app": "Slack", "displayId": 1,
                "frame": {"x": 0.0, "y": 0.0, "width": 100.0, "height": 100.0, "scale": 1.0}},
            "transcript": {"source": "none", "text": "", "segments": []},
            "strokes": [], "marks": [], "events": [],
            "keyframes": [{"index": 1, "t": 4_000, "reason": "click",
                "file": "frames/frame-01.png", "event": "clicked"}],
            "contactSheet": "contact-sheet.png"
        });
        fs::write(bundle.join("scrawl.json"), manifest.to_string()).unwrap();
        dir
    }

    async fn start(captures: &Path) -> String {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let service = Service {
            token: TOKEN.to_owned(),
            captures: captures.to_owned(),
        };
        tokio::spawn(serve(listener, Arc::new(service)));
        url
    }

    async fn post(url: &str, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(url)
            .bearer_auth(TOKEN)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn call(url: &str, id: u64, name: &str, arguments: Value) -> Value {
        let request = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": arguments}});
        post(url, request).await.json().await.unwrap()
    }

    #[tokio::test]
    async fn speaks_mcp_and_reads_a_capture() {
        let captures = captures();
        let url = start(captures.path()).await;

        let init = post(
            &url,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "test", "version": "1"}}}),
        )
        .await;
        assert_eq!(init.status(), StatusCode::OK);
        let init: Value = init.json().await.unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert!(init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("get_capture"));

        let initialized = post(
            &url,
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )
        .await;
        assert_eq!(initialized.status(), StatusCode::ACCEPTED);

        let list: Value = post(
            &url,
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        )
        .await
        .json()
        .await
        .unwrap();
        let names: Vec<_> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["get_capture", "get_frames", "list_captures", "get_manifest"]
        );

        // The path from a paste on this Mac, as an agent elsewhere passes it.
        let capture = call(
            &url,
            3,
            "get_capture",
            json!({"capture": "/Users/someone/captures/scrawl-1-a.scrawl"}),
        )
        .await;
        let content = &capture["result"]["content"];
        assert!(content[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("[scrawl 0:07 · Slack · scrawl-1-a]"));
        assert_eq!(content[1]["mimeType"], "image/png");
        let png = base64::engine::general_purpose::STANDARD
            .decode(content[1]["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            image::load_from_memory(&png).unwrap().dimensions(),
            (1600, 50)
        );

        let frames = call(&url, 4, "get_frames", json!({"keyframes": [1]})).await;
        assert_eq!(frames["result"]["content"][0]["text"], "#1 (0:04)");
        assert_eq!(frames["result"]["content"][1]["type"], "image");

        let listed = call(&url, 5, "list_captures", json!({})).await;
        assert!(listed["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("scrawl-1-a · "));

        let manifest = call(&url, 6, "get_manifest", json!({"capture": "latest"})).await;
        assert!(manifest["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"keyframes\""));

        // A failing tool is a result the agent reads; a missing tool or method is an error.
        let missing = call(&url, 7, "get_frames", json!({"keyframes": [9]})).await;
        assert_eq!(missing["result"]["isError"], true);
        assert!(missing["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("No keyframe #9"));
        let unknown = call(&url, 8, "nope", json!({})).await;
        assert_eq!(unknown["error"]["code"], -32602);
        let method: Value = post(
            &url,
            json!({"jsonrpc": "2.0", "id": 9, "method": "resources/list"}),
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(method["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn refuses_requests_without_the_token_or_from_web_pages() {
        let captures = captures();
        let url = start(captures.path()).await;
        let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
        let client = reqwest::Client::new();
        let status = |request: reqwest::RequestBuilder| async move {
            request.send().await.unwrap().status()
        };

        assert_eq!(
            status(client.post(&url).json(&ping)).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(client.post(&url).bearer_auth("secreT").json(&ping)).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(
                client
                    .post(&url)
                    .bearer_auth(TOKEN)
                    .header("Origin", "http://evil.example")
                    .json(&ping)
            )
            .await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(
                client
                    .post(&url)
                    .bearer_auth(TOKEN)
                    .header("Origin", "http://localhost:3000")
                    .json(&ping)
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            status(client.get(&url).bearer_auth(TOKEN)).await,
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            status(
                client
                    .post(url.replace("/mcp", "/other"))
                    .bearer_auth(TOKEN)
            )
            .await,
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn tailscale_range() {
        assert!(is_tailscale(Ipv4Addr::new(100, 64, 0, 1)));
        assert!(is_tailscale(Ipv4Addr::new(100, 127, 255, 254)));
        assert!(!is_tailscale(Ipv4Addr::new(100, 128, 0, 1)));
        assert!(!is_tailscale(Ipv4Addr::new(100, 63, 0, 1)));
        assert!(!is_tailscale(Ipv4Addr::new(10, 64, 0, 1)));
    }

    #[test]
    fn loopback_origins() {
        let loopback = |origin: &str| is_loopback_origin(&HeaderValue::from_str(origin).unwrap());
        assert!(loopback("http://127.0.0.1:47790"));
        assert!(loopback("http://localhost"));
        assert!(loopback("http://[::1]:8080"));
        assert!(!loopback("http://localhost.evil.example"));
        assert!(!loopback("http://127.0.0.1.evil.example"));
        assert!(!loopback("null"));
    }
}
