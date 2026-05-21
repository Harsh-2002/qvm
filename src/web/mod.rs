//! Embedded web UI for `qvm web`.
//!
//! Architectural rule, same shape as the TUI: this is a thin presenter over
//! the same `commands::*` functions the CLI uses. No parallel business logic.
//!
//! The server is sync (`tiny_http`). VNC consoles are wired by spawning
//! `websockify` once per VM on first request and iframing the noVNC URL
//! served by it. websockify children are tracked and killed when `qvm web`
//! exits.
//!
//! No authentication: `qvm web` is invoked on demand by the operator, runs
//! in the foreground, and exits on Ctrl-C. Defaults to binding 127.0.0.1.

mod assets;
mod templates;

use crate::cmd::{have, require};
use crate::commands;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt;
use crate::tui::app::VmRow;
use std::collections::HashMap;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server};

// ── flash messages ───────────────────────────────────────────────────────────
//
// We need to communicate "Started 'web01'" or "delete failed: …" across a
// POST→303→GET redirect. The classic web pattern is a flash cookie: set on
// the redirect response, read+cleared on the next render. Keeps URLs clean
// and doesn't require client-side state.

/// Two-tuple level + message. Level is "ok" or "err".
type Flash = (String, String);

fn read_flash(req: &Request) -> Option<Flash> {
    for h in req.headers() {
        if !h.field.equiv("cookie") { continue; }
        for kv in h.value.as_str().split(';') {
            let kv = kv.trim();
            if let Some(v) = kv.strip_prefix("qvm_flash=") {
                let dec = url_decode(v);
                if let Some(i) = dec.find(':') {
                    return Some((dec[..i].to_string(), dec[i+1..].to_string()));
                }
            }
        }
    }
    None
}

fn flash_set_header(level: &str, msg: &str) -> Header {
    let value = format!("{}:{}", level, url_encode(msg));
    let cookie = format!("qvm_flash={}; Path=/; Max-Age=15; HttpOnly; SameSite=Lax", value);
    Header::from_bytes("Set-Cookie", cookie).unwrap()
}

fn flash_clear_header() -> Header {
    Header::from_bytes("Set-Cookie", "qvm_flash=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax").unwrap()
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Per-VM websockify session: which TCP port the bridge listens on, and the
/// child handle so we can kill it on shutdown.
struct WsSession {
    port:  u16,
    child: Child,
}
impl Drop for WsSession {
    fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
}

struct State {
    cfg:      Config,
    ws_dir:   String,                       // /usr/share/novnc (or alt)
    sessions: Mutex<HashMap<String, WsSession>>,
}

pub fn run(cfg: Config, bind: &str, port: u16) -> Result<()> {
    let novnc = ["/usr/share/novnc", "/usr/share/webapps/novnc", "/usr/share/noVNC"]
        .iter()
        .find(|p| Path::new(p).join("vnc_lite.html").exists())
        .copied()
        .map(str::to_string);

    // Pre-flight: console route needs both. Don't hard-fail the whole UI if
    // they're missing — the rest of qvm web still works — but warn loudly so
    // the user knows the console tab won't function.
    let console_ok = novnc.is_some() && have("websockify");
    if !console_ok {
        eprintln!(
            "warning: websockify and/or novnc not installed; the VM console page \
             will not load. apt-get install websockify novnc"
        );
    }

    let addr = format!("{bind}:{port}");
    let server = Server::http(&addr).map_err(|e|
        Error::User(format!("cannot bind {addr}: {e}")))?;

    println!("qvm web listening on http://{}", display_addr(bind, port));
    println!("(Ctrl-C to stop)");

    let state = Arc::new(State {
        cfg,
        ws_dir: novnc.unwrap_or_default(),
        sessions: Mutex::new(HashMap::new()),
    });

    // Shutdown flag flipped by SIGINT.
    let shutdown = Arc::new(AtomicBool::new(false));
    register_sigint(shutdown.clone());

    while !shutdown.load(Ordering::SeqCst) {
        match server.recv_timeout(Duration::from_millis(250)) {
            Ok(Some(req)) => {
                let st = state.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle(req, &st) {
                        eprintln!("request error: {e}");
                    }
                });
            }
            Ok(None) => {}
            Err(_)   => {}
        }
    }

    // Cleanup: WsSession::drop() kills + waits each child.
    println!("\nShutting down. Cleaning up websockify sessions...");
    drop(state); // drops sessions map → drops each WsSession → kills child.
    Ok(())
}

fn display_addr(bind: &str, port: u16) -> String {
    let host = if bind == "0.0.0.0" {
        detect_lan_ip().unwrap_or_else(|| bind.to_string())
    } else {
        bind.to_string()
    };
    format!("{host}:{port}")
}

fn detect_lan_ip() -> Option<String> {
    let out = crate::cmd::run("hostname", ["-I"]).ok()?;
    out.split_whitespace()
        .find(|s| !s.starts_with("127.") && s.contains('.'))
        .map(str::to_string)
}

fn register_sigint(flag: Arc<AtomicBool>) {
    // We piggyback on signal-hook (already pulled in via crossterm) so this
    // doesn't add a new dep. If installation fails, fall back to relying on
    // Ctrl-C interrupting tiny_http's poll.
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, flag);
}

// ── routing ──────────────────────────────────────────────────────────────────

fn handle(req: Request, state: &State) -> Result<()> {
    let method = req.method().clone();
    // tiny_http includes the query string in `.url()`.
    let raw_url = req.url().to_string();
    let (path, query) = split_query(&raw_url);

    match (&method, path.as_str()) {
        (Method::Get, "/")            => index(req, state, &query),
        (Method::Get, "/static/style.css") => serve_static(req, "text/css; charset=utf-8", assets::STYLE_CSS.as_bytes()),
        (Method::Get, "/static/app.js")    => serve_static(req, "application/javascript", assets::APP_JS.as_bytes()),
        (Method::Get, "/vm/new")      => respond_html(req, 200, templates::create_form(&state.cfg, None)),
        (Method::Post,"/vm/new")      => create_vm(req, state),
        (Method::Get, p) if p.starts_with("/vm/") => match split_vm_path(p) {
            VmPath::Inspect(n)       => inspect(req, state, &n),
            VmPath::DeleteRoute(n)   => respond_html(req, 200, templates::delete_confirm(&n)),
            VmPath::Console(n)       => console(req, state, &n),
            _                        => not_found(req),
        },
        (Method::Post, p) if p.starts_with("/vm/") => match split_vm_path(p) {
            VmPath::Action(n, "start")    => act(req, &n, libvirt::start),
            VmPath::Action(n, "stop")     => act(req, &n, libvirt::shutdown),
            VmPath::Action(n, "restart")  => act(req, &n, libvirt::reboot),
            VmPath::Action(n, "kill")     => act(req, &n, libvirt::destroy),
            VmPath::DeleteRoute(n)        => delete(req, state, &n),
            _                             => not_found(req),
        },
        _ => not_found(req),
    }
}

enum VmPath {
    Inspect(String),
    DeleteRoute(String),         // /vm/<n>/delete  (GET=confirm, POST=execute)
    Action(String, &'static str),// POST /vm/<n>/{start,stop,restart,kill}
    Console(String),
    Unknown,
}
fn split_vm_path(p: &str) -> VmPath {
    let rest = &p["/vm/".len()..];
    let mut parts = rest.splitn(2, '/');
    let name = parts.next().unwrap_or("").to_string();
    if name.is_empty() { return VmPath::Unknown; }
    let sub = parts.next();
    match sub {
        None              => VmPath::Inspect(name),
        Some("delete")    => VmPath::DeleteRoute(name),
        Some("console")   => VmPath::Console(name),
        Some("start")     => VmPath::Action(name, "start"),
        Some("stop")      => VmPath::Action(name, "stop"),
        Some("restart")   => VmPath::Action(name, "restart"),
        Some("kill")      => VmPath::Action(name, "kill"),
        Some(_)           => VmPath::Unknown,
    }
}

// ── handlers ────────────────────────────────────────────────────────────────

fn index(req: Request, _state: &State, query: &str) -> Result<()> {
    let rows = list_vms();
    if query.contains("fragment=grid") {
        // Fragment endpoint for the 2s auto-refresh — no toast, no shell.
        return respond_html(req, 200, templates::grid_only(&rows));
    }
    let flash = read_flash(&req);
    respond_html_with_flash(req, 200, templates::home_page(&rows, flash.as_ref()), flash.is_some())
}

fn list_vms() -> Vec<VmRow> {
    let domains = libvirt::domains().unwrap_or_default();
    domains.into_iter().map(|d| VmRow {
        ip: if d.state == "running" { libvirt::ipv4(&d.name) } else { None },
        name: d.name,
        state: d.state,
    }).collect()
}

fn create_vm(mut req: Request, state: &State) -> Result<()> {
    let body = read_body(&mut req)?;
    let form = parse_form(&body);

    let name      = form.get("name").cloned().unwrap_or_default();
    let distro    = form.get("distro").cloned();
    let cpus      = form.get("cpus").and_then(|v| v.parse().ok());
    let memory_gb = form.get("memory_gb").and_then(|v| v.parse().ok());
    let disk_gb   = form.get("disk_gb").and_then(|v| v.parse().ok());
    let user      = form.get("user").cloned().filter(|s| !s.is_empty());

    let args = commands::create::Args {
        name: name.clone(),
        distro,
        cpus,
        memory_gb,
        disk_gb,
        user,
        password: None,
        no_autostart: false,
    };

    match commands::create::run(&state.cfg, args) {
        Ok(()) => redirect_with_flash(req, "/", "ok", &format!("Created '{name}'")),
        Err(e) => respond_html(req, 400, templates::create_form(&state.cfg, Some(&e.to_string()))),
    }
}

fn inspect(req: Request, _state: &State, name: &str) -> Result<()> {
    if !crate::util::valid_vm_name(name) { return not_found(req); }
    match libvirt::dominfo(name) {
        Ok(s)  => respond_html(req, 200, templates::inspect_page(name, &s)),
        Err(e) => respond_html(req, 404, templates::error_page(404, &e.to_string())),
    }
}

fn delete(req: Request, state: &State, name: &str) -> Result<()> {
    if !crate::util::valid_vm_name(name) { return not_found(req); }
    // If a console session exists for this VM, kill it before delete.
    {
        let mut map = state.sessions.lock().unwrap();
        map.remove(name);
    }
    match commands::delete::run(&state.cfg, name, /* force */ true) {
        Ok(()) => redirect_with_flash(req, "/", "ok",  &format!("Deleted '{name}'")),
        Err(e) => redirect_with_flash(req, "/", "err", &format!("delete '{name}' failed: {e}")),
    }
}

fn act(req: Request, name: &str, f: fn(&str) -> Result<()>) -> Result<()> {
    if !crate::util::valid_vm_name(name) { return not_found(req); }
    match f(name) {
        Ok(()) => redirect_with_flash(req, "/", "ok",  &format!("Action sent to '{name}'")),
        Err(e) => redirect_with_flash(req, "/", "err", &format!("action on '{name}' failed: {e}")),
    }
}

fn console(req: Request, state: &State, name: &str) -> Result<()> {
    if !crate::util::valid_vm_name(name) { return not_found(req); }
    libvirt::require_running(name).map_err(|e| Error::User(e.to_string()))?;
    let ep = libvirt::vnc_endpoint(name)
        .ok_or_else(|| Error::User(format!("'{name}' has no VNC display")))?;

    require("websockify")?;
    if state.ws_dir.is_empty() {
        return respond_html(req, 500, templates::error_page(
            500, "noVNC not installed (apt-get install novnc)"
        ));
    }

    let ws_port = ensure_session(state, name, ep.port)?;
    // The iframe URL must use the same hostname the browser is already on,
    // otherwise mixed-host security errors trip the WebSocket. Pull Host
    // from the request.
    let host = req_host(&req).unwrap_or_else(|| "127.0.0.1".to_string());
    let host_only = host.split(':').next().unwrap_or("127.0.0.1").to_string();
    respond_html(req, 200, templates::console_page(name, ws_port, &host_only))
}

fn ensure_session(state: &State, vm: &str, vnc_port: u16) -> Result<u16> {
    let mut map = state.sessions.lock().unwrap();
    if let Some(s) = map.get(vm) { return Ok(s.port); }

    let port = next_free_port(6080).ok_or_else(||
        Error::User("no free TCP port for websockify in 6080..6200".into()))?;

    let bind = &state.cfg.vnc.bind;
    let dial_host = if bind == "0.0.0.0" { "127.0.0.1" } else { bind.as_str() };

    let child = Command::new("websockify")
        .args([
            "--web", &state.ws_dir,
            &format!("0.0.0.0:{port}"),
            &format!("{dial_host}:{vnc_port}"),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| Error::User(format!("failed to spawn websockify: {e}")))?;

    // Give websockify a moment to bind.
    std::thread::sleep(Duration::from_millis(250));
    map.insert(vm.to_string(), WsSession { port, child });
    Ok(port)
}

fn next_free_port(start: u16) -> Option<u16> {
    (start..(start + 120)).find(|&p| TcpListener::bind(("0.0.0.0", p)).is_ok())
}

// ── HTTP helpers ─────────────────────────────────────────────────────────────

fn split_query(url: &str) -> (String, String) {
    match url.find('?') {
        Some(i) => (url[..i].to_string(), url[i+1..].to_string()),
        None    => (url.to_string(), String::new()),
    }
}

fn read_body(req: &mut Request) -> Result<String> {
    let mut s = String::new();
    req.as_reader().read_to_string(&mut s)
        .map_err(|e| Error::User(format!("read body: {e}")))?;
    Ok(s)
}

fn parse_form(body: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for kv in body.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = match kv.find('=') {
            Some(i) => (&kv[..i], &kv[i+1..]),
            None    => (kv, ""),
        };
        out.insert(url_decode(k), url_decode(v));
    }
    out
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => { out.push(b' '); i += 1; }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex(bytes[i+1]), hex(bytes[i+2])) {
                    out.push((hi << 4 | lo) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => { out.push(b); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u32> {
    Some(match b {
        b'0'..=b'9' => (b - b'0') as u32,
        b'a'..=b'f' => 10 + (b - b'a') as u32,
        b'A'..=b'F' => 10 + (b - b'A') as u32,
        _ => return None,
    })
}

fn req_host(req: &Request) -> Option<String> {
    for h in req.headers() {
        if h.field.equiv("host") {
            return Some(h.value.as_str().to_string());
        }
    }
    None
}

fn respond_html(req: Request, status: u16, body: String) -> Result<()> {
    respond_html_with_flash(req, status, body, false)
}

/// Render an HTML response. When `clear_flash` is true, also instruct the
/// browser to drop the qvm_flash cookie (we just rendered its content).
fn respond_html_with_flash(req: Request, status: u16, body: String, clear: bool) -> Result<()> {
    let mut resp = Response::from_string(body)
        .with_status_code(status as u32);
    resp.add_header(Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap());
    resp.add_header(Header::from_bytes("Cache-Control", "no-store").unwrap());
    if clear { resp.add_header(flash_clear_header()); }
    req.respond(resp).ok();
    Ok(())
}

fn serve_static(req: Request, ctype: &str, body: &'static [u8]) -> Result<()> {
    let mut resp = Response::from_data(body);
    resp.add_header(Header::from_bytes("Content-Type", ctype).unwrap());
    resp.add_header(Header::from_bytes("Cache-Control", "public, max-age=3600").unwrap());
    req.respond(resp).ok();
    Ok(())
}

fn redirect_with_flash(req: Request, target: &str, level: &str, msg: &str) -> Result<()> {
    let mut resp = Response::from_string("")
        .with_status_code(303);
    resp.add_header(Header::from_bytes("Location", target).unwrap());
    resp.add_header(flash_set_header(level, msg));
    req.respond(resp).ok();
    Ok(())
}

fn not_found(req: Request) -> Result<()> {
    respond_html(req, 404, templates::error_page(404, "Not found"))
}

// ── trim — silence unused warnings for items only referenced in this file ──

#[allow(dead_code)]
fn _ensure_traits_compile<T>(_: T) {}
