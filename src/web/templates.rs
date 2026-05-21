//! HTML rendering functions. Pure: take state, return strings. No I/O.
//!
//! We don't use a templating engine — every page is a small Rust function
//! that builds an HTML string. Keeps the dep tree narrow and the rendered
//! output trivial to debug.

use crate::config::Config;
use crate::tui::app::VmRow; // re-use the same row shape the TUI uses

const VERSION: &str = env!("CARGO_PKG_VERSION");

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

fn shell(title: &str, body: &str, summary: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title} · qvm</title>
  <link rel="stylesheet" href="/static/style.css">
</head>
<body>
<header>
  <div class="brand"><a href="/">qvm</a><span class="ver">{ver}</span></div>
  <div class="summary">{summary}</div>
</header>
<main>
{body}
</main>
<footer>qvm {ver} — interactive VM management</footer>
<script src="/static/app.js"></script>
</body>
</html>
"#,
        title = esc(title),
        ver = VERSION,
        summary = summary,
        body = body,
    )
}

fn vm_card(r: &VmRow) -> String {
    let state_class = r.state.replace(' ', "-");
    let ip = r.ip.as_deref().unwrap_or("—");
    let running = r.state == "running";
    let lifecycle_btns = if running {
        format!(
            r#"<form method="post" action="/vm/{n}/stop" style="display:inline">
                 <button class="btn btn-sm">Stop</button>
               </form>
               <form method="post" action="/vm/{n}/restart" style="display:inline">
                 <button class="btn btn-sm">Restart</button>
               </form>
               <a class="btn btn-sm" href="/vm/{n}/console">Console</a>"#,
            n = esc(&r.name),
        )
    } else {
        format!(
            r#"<form method="post" action="/vm/{n}/start" style="display:inline">
                 <button class="btn btn-sm primary">Start</button>
               </form>"#,
            n = esc(&r.name),
        )
    };

    format!(
        r#"<div class="card">
  <div class="row">
    <div class="name">{name}</div>
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
          <p>No VMs yet.</p>
          <p><a class="btn primary" href="/vm/new">+ Create your first VM</a></p>
        </div>"#.to_string();
    }
    rows.iter().map(vm_card).collect::<Vec<_>>().join("\n")
}

pub fn home_page(rows: &[VmRow]) -> String {
    let running = rows.iter().filter(|r| r.state == "running").count();
    let summary = format!("{} VMs ({} running)", rows.len(), running);
    let grid = grid_only(rows);
    let body = format!(
        r#"<div class="toolbar">
  <h1 style="margin:0;font-size:1.4rem">Virtual machines</h1>
  <a class="btn primary" href="/vm/new">+ New VM</a>
</div>
<div class="grid" data-vm-grid>
{grid}
</div>"#,
    );
    shell("VMs", &body, &esc(&summary))
}

pub fn create_form(cfg: &Config, error: Option<&str>) -> String {
    let mut distro_opts = String::new();
    for k in cfg.distros.keys() {
        let selected = if k == &cfg.defaults.distro { " selected" } else { "" };
        distro_opts.push_str(&format!(
            r#"<option value="{k}"{sel}>{k}</option>"#,
            k = esc(k), sel = selected,
        ));
    }
    let err_html = match error {
        Some(e) => format!(r#"<div class="toast err">{}</div>"#, esc(e)),
        None    => String::new(),
    };
    let body = format!(
        r#"<div class="toolbar">
  <h1 style="margin:0;font-size:1.4rem">Create VM</h1>
  <a class="btn" href="/">Cancel</a>
</div>
<form class="stacked" method="post" action="/vm/new">
  <label>Name
    <input name="name" required pattern="[A-Za-z0-9][A-Za-z0-9._-]*" autofocus autocomplete="off">
    <span class="hint">Letters, digits, . - _ (must start alnum).</span>
  </label>
  <label>Distro
    <select name="distro">{distro_opts}</select>
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
  <div style="display:flex;gap:0.5rem;justify-content:flex-end">
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
    shell("Create VM", &body, "")
}

pub fn inspect_page(name: &str, dominfo: &str) -> String {
    let body = format!(
        r#"<div class="toolbar">
  <h1 style="margin:0;font-size:1.4rem">{name}</h1>
  <a class="btn" href="/">← Back</a>
</div>
<pre class="dominfo">{di}</pre>"#,
        name = esc(name),
        di = esc(dominfo),
    );
    shell(name, &body, "")
}

pub fn console_page(name: &str, ws_port: u16, host: &str) -> String {
    let src = format!(
        "http://{host}:{ws_port}/vnc_lite.html?host={host}&port={ws_port}&autoconnect=true&resize=scale&reconnect=true",
    );
    let body = format!(
        r#"<div class="toolbar">
  <h1 style="margin:0;font-size:1.4rem">Console — {name}</h1>
  <a class="btn" href="/">← Back</a>
</div>
<iframe class="console-frame" src="{src}" allow="clipboard-read; clipboard-write"></iframe>
<p class="hint" style="margin-top:0.5rem">If the console doesn't load, ensure <code>websockify</code> and <code>novnc</code> are installed on this host.</p>"#,
        name = esc(name),
        src = esc(&src),
    );
    shell(&format!("Console: {name}"), &body, "")
}

pub fn delete_confirm(name: &str) -> String {
    let body = format!(
        r#"<div class="toolbar">
  <h1 style="margin:0;font-size:1.4rem">Delete VM</h1>
  <a class="btn" href="/">Cancel</a>
</div>
<div class="card" style="max-width:520px">
  <p>Are you sure you want to delete <strong>{name}</strong>?</p>
  <p class="hint">This permanently removes the VM, its disk, and its cloud-init seed. This action cannot be undone.</p>
  <form method="post" action="/vm/{name_attr}/delete">
    <div style="display:flex;gap:0.5rem;justify-content:flex-end">
      <a class="btn" href="/">Cancel</a>
      <button class="btn danger">Delete permanently</button>
    </div>
  </form>
</div>"#,
        name = esc(name),
        name_attr = esc(name),
    );
    shell(&format!("Delete: {name}"), &body, "")
}

pub fn error_page(status: u16, msg: &str) -> String {
    let body = format!(
        r#"<div class="card" style="max-width:640px;margin:2rem auto">
  <h1 style="margin:0 0 0.5rem 0;font-size:1.4rem">Error {status}</h1>
  <p class="hint">{msg}</p>
  <p><a class="btn" href="/">← Home</a></p>
</div>"#,
        msg = esc(msg),
    );
    shell(&format!("Error {status}"), &body, "")
}
