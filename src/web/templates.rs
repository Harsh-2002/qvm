//! HTML rendering functions. Pure: take state, return strings. No I/O.
//!
//! We don't use a templating engine — every page is a small Rust function
//! that builds an HTML string. Keeps the dep tree narrow and the rendered
//! output trivial to debug.

use crate::config::Config;
use crate::tui::app::VmRow; // re-use the same row shape the TUI uses

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Cookie-flash tuple from mod.rs: (level, message).
pub type Flash<'a> = &'a (String, String);

/// HTML-escape a string for safe interpolation into element content/attrs.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&'  => out.push_str("&amp;"),
            '<'  => out.push_str("&lt;"),
            '>'  => out.push_str("&gt;"),
            '"'  => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _    => out.push(c),
        }
    }
    out
}

fn shell(title: &str, body: &str, summary: &str, flash: Option<Flash>) -> String {
    let toast = flash.map(|(level, msg)| {
        let class = if level == "err" { "toast err" } else { "toast ok" };
        format!(
            r#"<div class="{class}" id="toast" role="status">{msg}</div>"#,
            msg = esc(msg),
        )
    }).unwrap_or_default();

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title} · qvm</title>
  <link rel="stylesheet" href="/static/style.css?v={ver}">
  <link rel="icon" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'%3E%3Crect width='100' height='100' rx='20' fill='%233b82f6'/%3E%3Ctext x='50' y='66' font-family='monospace' font-size='52' font-weight='bold' text-anchor='middle' fill='white'%3Eq%3C/text%3E%3C/svg%3E">
</head>
<body>
<header>
  <div class="brand"><a href="/">qvm</a><span class="ver">{ver}</span></div>
  <div class="summary">{summary}</div>
</header>
<main>
{body}
</main>
{toast}
<footer>qvm {ver} — press <kbd>?</kbd> for shortcuts</footer>
<script src="/static/app.js?v={ver}"></script>
</body>
</html>
"#,
        title = esc(title),
        ver = VERSION,
        summary = summary,
        body = body,
        toast = toast,
    )
}

fn vm_card(r: &VmRow) -> String {
    let state_class = r.state.replace(' ', "-");
    let ip = r.ip.as_deref().unwrap_or("—");
    let running = r.state == "running";
    let lifecycle_btns = if running {
        format!(
            r#"<form method="post" action="/vm/{n}/stop" class="inline">
                 <button class="btn btn-sm">Stop</button>
               </form>
               <form method="post" action="/vm/{n}/restart" class="inline">
                 <button class="btn btn-sm">Restart</button>
               </form>
               <a class="btn btn-sm" href="/vm/{n}/console">Console</a>"#,
            n = esc(&r.name),
        )
    } else {
        format!(
            r#"<form method="post" action="/vm/{n}/start" class="inline">
                 <button class="btn btn-sm primary">Start</button>
               </form>"#,
            n = esc(&r.name),
        )
    };

    format!(
        r#"<div class="card" data-vm="{name_attr}">
  <div class="row">
    <a class="name" href="/vm/{name_attr}">{name}</a>
    <span class="badge {sc}">{state}</span>
  </div>
  <div class="meta">IP: {ip}</div>
  <div class="actions">
    <a class="btn btn-sm" href="/vm/{name_attr}">Inspect</a>
    {lifecycle}
    <a class="btn btn-sm danger" href="/vm/{name_attr}/delete">Delete</a>
  </div>
</div>"#,
        name = esc(&r.name),
        name_attr = esc(&r.name),
        state = esc(&r.state),
        sc = esc(&state_class),
        ip = esc(ip),
        lifecycle = lifecycle_btns,
    )
}

pub fn grid_only(rows: &[VmRow]) -> String {
    if rows.is_empty() {
        return r#"<div class="empty">
          <p style="font-size:1.1rem;margin:0 0 0.5rem 0">No VMs yet.</p>
          <p class="hint" style="margin:0 0 1rem 0">Create one with the button below, or run <code>qvm run NAME</code> on the host.</p>
          <p><a class="btn primary" href="/vm/new">+ Create your first VM</a></p>
        </div>"#.to_string();
    }
    rows.iter().map(vm_card).collect::<Vec<_>>().join("\n")
}

pub fn home_page(rows: &[VmRow], flash: Option<Flash>) -> String {
    let running = rows.iter().filter(|r| r.state == "running").count();
    let summary = format!("{} VMs ({} running)", rows.len(), running);
    let grid = grid_only(rows);
    let body = format!(
        r#"<div class="toolbar">
  <h1 class="title">Virtual machines</h1>
  <a class="btn primary" href="/vm/new" data-shortcut="n">+ New VM</a>
</div>
<div class="grid" data-vm-grid>
{grid}
</div>"#,
    );
    shell("VMs", &body, &esc(&summary), flash)
}

pub fn create_form(cfg: &Config, error: Option<&str>) -> String {
    let mut distro_opts = String::new();
    for (k, _) in cfg.distros.iter() {
        let pulled = cfg.image_path(k).map(|p| p.exists()).unwrap_or(false);
        let selected = if k == &cfg.defaults.distro { " selected" } else { "" };
        let badge = if pulled { "● pulled" } else { "○ not pulled (will auto-download)" };
        distro_opts.push_str(&format!(
            r#"<option value="{k}"{sel}>{k}  —  {badge}</option>"#,
            k = esc(k), sel = selected, badge = badge,
        ));
    }
    let err_html = match error {
        Some(e) => format!(r#"<div class="toast err" style="position:static;margin-top:1rem;max-width:520px">{}</div>"#, esc(e)),
        None    => String::new(),
    };
    let body = format!(
        r#"<div class="toolbar">
  <h1 class="title">Create VM</h1>
  <a class="btn" href="/">Cancel</a>
</div>
<form class="stacked" method="post" action="/vm/new" data-form="create">
  <label>Name
    <input name="name" required pattern="[A-Za-z0-9][A-Za-z0-9._-]*" autofocus autocomplete="off">
    <span class="hint">Letters, digits, . - _ (must start alnum).</span>
  </label>
  <label>Distro
    <select name="distro">{distro_opts}</select>
    <span class="hint">"Not pulled" distros download automatically (~500 MB – 1 GB, takes a minute or two).</span>
  </label>
  <label>vCPUs
    <input name="cpus" type="number" min="1" max="256" value="{cpus}" required>
  </label>
  <label>RAM (GB)
    <input name="memory_gb" type="number" min="1" max="1024" value="{mem}" required>
  </label>
  <label>Disk (GB)
    <input name="disk_gb" type="number" min="1" max="10000" value="{disk}" required>
  </label>
  <label>Login user (optional)
    <input name="user" placeholder="auto: vmXXXXXX" autocomplete="off">
    <span class="hint">Leave blank for a randomly-generated username.</span>
  </label>
  <div class="form-actions">
    <a class="btn" href="/">Cancel</a>
    <button class="btn primary">Create</button>
  </div>
</form>
{err}"#,
        cpus = cfg.defaults.cpus,
        mem  = cfg.defaults.memory_gb,
        disk = cfg.defaults.disk_gb,
        err  = err_html,
    );
    shell("Create VM", &body, "", None)
}

/// Parse `virsh dominfo` lines into key/value pairs for tabular rendering.
/// Whitespace inside values (e.g. "9.6s") is preserved; leading/trailing
/// whitespace is trimmed.
fn parse_dominfo(s: &str) -> Vec<(String, String)> {
    s.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() { return None; }
            line.find(':').map(|i| {
                (line[..i].trim().to_string(), line[i+1..].trim().to_string())
            })
        })
        .collect()
}

fn state_badge(state: &str) -> String {
    let class = state.replace(' ', "-");
    format!(r#"<span class="badge {cls}">{txt}</span>"#, cls = esc(&class), txt = esc(state))
}

fn humanize_kib(s: &str) -> Option<String> {
    let trimmed = s.trim_end_matches(" KiB");
    if trimmed == s { return None; }
    let n: u64 = trimmed.parse().ok()?;
    let gib = n as f64 / 1024.0 / 1024.0;
    let mib = n as f64 / 1024.0;
    if gib >= 1.0 { Some(format!("{n} KiB  ({:.2} GiB)", gib)) }
    else if mib >= 1.0 { Some(format!("{n} KiB  ({:.0} MiB)", mib)) }
    else { None }
}

pub fn inspect_page(name: &str, dominfo: &str) -> String {
    let rows = parse_dominfo(dominfo);
    let mut tbl = String::new();
    for (k, v) in &rows {
        let rendered = match k.as_str() {
            "State" => state_badge(v),
            "Max memory" | "Used memory" => humanize_kib(v).unwrap_or_else(|| esc(v)),
            _ => esc(v),
        };
        tbl.push_str(&format!(
            r#"<tr><th>{k}</th><td>{v}</td></tr>"#,
            k = esc(k), v = rendered,
        ));
    }

    let body = format!(
        r#"<div class="toolbar">
  <h1 class="title">{name}</h1>
  <a class="btn" href="/">← Back</a>
</div>
<table class="kv">
<tbody>{tbl}</tbody>
</table>
<details style="margin-top:1.5rem">
  <summary class="hint" style="cursor:pointer">Raw <code>virsh dominfo</code> output</summary>
  <pre class="dominfo">{raw}</pre>
</details>"#,
        name = esc(name),
        tbl = tbl,
        raw = esc(dominfo),
    );
    shell(name, &body, "", None)
}

pub fn console_page(name: &str, ws_port: u16, host: &str) -> String {
    let src = format!(
        "http://{host}:{ws_port}/vnc_lite.html?host={host}&port={ws_port}&autoconnect=true&resize=scale&reconnect=true",
    );
    let body = format!(
        r#"<div class="toolbar">
  <h1 class="title">Console — {name}</h1>
  <a class="btn" href="/">← Back</a>
</div>
<iframe class="console-frame" src="{src}" allow="clipboard-read; clipboard-write"></iframe>
<p class="hint" style="margin-top:0.5rem">Click inside the console to capture keyboard. If it doesn't load, ensure <code>websockify</code> and <code>novnc</code> are installed on the host.</p>"#,
        name = esc(name),
        src = esc(&src),
    );
    shell(&format!("Console: {name}"), &body, "", None)
}

pub fn delete_confirm(name: &str) -> String {
    let body = format!(
        r#"<div class="toolbar">
  <h1 class="title">Delete VM</h1>
  <a class="btn" href="/">Cancel</a>
</div>
<div class="card" style="max-width:520px">
  <p>Are you sure you want to delete <strong>{name}</strong>?</p>
  <p class="hint">This permanently removes the VM, its disk, and its cloud-init seed. This action cannot be undone.</p>
  <form method="post" action="/vm/{name_attr}/delete">
    <div class="form-actions">
      <a class="btn" href="/">Cancel</a>
      <button class="btn danger">Delete permanently</button>
    </div>
  </form>
</div>"#,
        name = esc(name),
        name_attr = esc(name),
    );
    shell(&format!("Delete: {name}"), &body, "", None)
}

pub fn error_page(status: u16, msg: &str) -> String {
    let body = format!(
        r#"<div class="card" style="max-width:640px;margin:2rem auto">
  <h1 class="title" style="margin-bottom:0.5rem">Error {status}</h1>
  <p class="hint">{msg}</p>
  <p><a class="btn" href="/">← Home</a></p>
</div>"#,
        msg = esc(msg),
    );
    shell(&format!("Error {status}"), &body, "", None)
}
