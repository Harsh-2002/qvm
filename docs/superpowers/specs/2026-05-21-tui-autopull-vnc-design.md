# qvm — interactive TUI, auto-pull, VNC fix (design)

Date: 2026-05-21
Status: draft, pending user approval

## Goals

Three independent changes shipped together because they share build/test cycles
on `aether`:

1. **Interactive TUI mode.** Bare `qvm` (no subcommand) opens a production-grade
   ratatui dashboard. The CLI remains primary; TUI is a thin wrapper that
   delegates to the same `commands::*` functions used today. Bonus: pressing
   `e` on a row drops into `virsh console <name>` inline.
2. **Auto-pull missing images on create** (docker `run` semantics). When
   `qvm run web01 ubuntu:22.04` finds the base image is absent, qvm prints
   `Unable to find image 'ubuntu:22.04' locally, pulling...` and pulls it
   before continuing instead of erroring with "Run: qvm pull …".
3. **Fix `qvm vnc` connect string.** The current output prints
   `vncviewer <bind>:<port>` (e.g. `:5900`), which most VNC clients
   interpret as "display 5900 → port 11800" and reject. Print the canonical
   `host:display` form (`:0`) and the unambiguous double-colon
   `host::port` (`::5900`) form alongside it.

## Non-goals

- Real embedded VT100 console panel inside the TUI (would need
  `portable-pty` + `vt100` and a few hundred LOC of edge cases). The
  suspend-and-exec console below covers the same need with ~30 LOC.
- VNC websocket proxy / noVNC integration baked into qvm. Out of scope —
  same reason as section 2 of CLAUDE.md.
- Snapshot / migration / clustering features (still out of scope).

## CLAUDE.md update

Section 2 currently lists "Web UI or TUI" as out of scope. We override that
intentionally. The doc will be updated in the same change:

- In-scope list gains: "Interactive TUI mode (bare `qvm`) — a thin wrapper
  over the same `commands::*` functions; the CLI is still the primary
  interface."
- Section 8's "No interactive wizard" note is stale (init wizard already
  exists). Replace with a current description of the TUI.
- New module-map entry: `src/tui/` (mod.rs / app.rs / ui.rs / events.rs).

## Architecture

### TUI module layout

```
src/tui/
├── mod.rs       // pub fn run(cfg) -> Result<()>: terminal init/teardown,
│                // main event/tick loop, mode dispatch.
├── app.rs       // App state: vm_rows, selected, mode (Table | CreateForm |
│                // DeleteConfirm | ConsoleOut), status_toast, last_error.
├── ui.rs        // pure render fns: draw_header, draw_table, draw_keybar,
│                // draw_create_modal, draw_confirm_modal, draw_error_toast.
└── events.rs    // pump crossterm events; map keys → app.dispatch(action).
```

`mod.rs` is the only file that touches `crossterm::terminal::enable_raw_mode`
/ `disable_raw_mode`. Every other file is pure logic and is unit-testable
without a TTY.

### Crates added

- `ratatui = "0.30"` (current at time of writing — verify in `cargo add`)
- `crossterm = "0.31"` — input + raw mode; ratatui's default backend.

Nothing else. Specifically:

- No `dialoguer` (init wizard keeps using `std::io::stdin().read_line`).
- No `tui-input` / `tui-textarea` — the create modal uses a small inline
  text-input helper inside `app.rs` (~60 LOC). Keeping the dep list narrow.
- No PTY/vt100 deps (suspend-and-exec console).

Binary size impact: rough estimate +700 KB stripped musl. Acceptable for
the feature.

### Event loop

```
loop {
    terminal.draw(|f| ui::draw(f, &app))?;
    match poll_event_with_timeout(200ms)? {
        Some(Key(k))  => app.on_key(k, cfg)?,
        Some(Resize)  => {} // ratatui auto-resizes
        None          => {} // timeout
    }
    if app.tick_due() {
        app.refresh_vms()?;
    }
    if app.should_quit { break; }
}
```

`tick_due()` returns true every 2s. `refresh_vms()` calls into the same
`virsh` wrappers in `src/libvirt.rs` that `qvm ls` and `qvm inspect` use —
no parallel parsers.

### Modes & key bindings

| Mode | Keys | Action |
|---|---|---|
| Table | ↑/↓, j/k | move selection |
| Table | c       | open Create modal |
| Table | s       | start selected (no-op if running) |
| Table | t       | stop selected (no-op if stopped) |
| Table | r       | restart selected |
| Table | d       | open Delete confirm |
| Table | v       | open VNC info popup (same text as `qvm vnc <name>`) |
| Table | i       | open Inspect popup (same text as `qvm inspect <name>`) |
| Table | e       | suspend TUI, exec `virsh console <name>`, restore on exit |
| Table | q, Esc  | quit |
| CreateForm | Tab / Shift+Tab | move between fields |
| CreateForm | Enter           | submit (if all required filled) |
| CreateForm | Esc             | cancel back to Table |
| DeleteConfirm | y | confirm delete → `commands::delete::run(.., force=true)` |
| DeleteConfirm | n, Esc | cancel |
| ConsolePopup (VNC/Inspect) | any | close |

Create modal fields (defaults from `cfg.defaults`):

```
Name      [____________]   (required, validated by util::valid_vm_name)
Distro    [debian:13   ]   (cycles through cfg.distros on ←/→)
vCPUs     [2  ]            (numeric, ≥1)
RAM (GB)  [4  ]            (numeric, ≥1)
Disk (GB) [50 ]            (numeric, ≥1)
User      [auto         ]  (blank = random vmXXXXXX, same as today)

[ Create ]  [ Cancel ]
```

On submit, the TUI shows a "Creating…" toast and **suspends** to inherit
stdout/stderr so `qemu-img convert -p`'s progress bar is visible (same as
today's CLI). When `commands::create::run` returns, the TUI restores and
shows a success/error toast.

### Suspend-and-exec helper

Used by the console action and by long-running create. Shape:

```rust
fn suspend_terminal<F: FnOnce() -> Result<R>, R>(
    terminal: &mut Terminal<...>, f: F
) -> Result<R> {
    crossterm::terminal::disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen, Show)?;
    let r = f();
    execute!(stdout(), EnterAlternateScreen, Hide)?;
    crossterm::terminal::enable_raw_mode()?;
    terminal.clear()?;
    r
}
```

Console action: `suspend_terminal(t, || cmd::run_inherit("virsh", ["console", name]))`.

## Auto-pull

In `commands::create::run`, replace:

```rust
if !base.exists() {
    return Err(Error::User(format!(
        "base image missing: {}\nRun: qvm pull {distro}", base.display()
    )));
}
```

With:

```rust
if !base.exists() {
    println!("Unable to find image '{distro}' locally, pulling...");
    crate::commands::pull::pull_one(cfg, &distro)?;
}
```

`pull_one` already exists (extracted earlier for `qvm init --pull-all`)
and writes to `<path>.partial` then renames — atomic. If it fails, the
create fails cleanly with the underlying pull error; no half-state on
disk.

No change to the TUI create flow: it calls the same `commands::create::run`.

## VNC fix

Current output (the bug):

```
VNC for 'Hermes':
  bind   : 10.1.1.10
  port   : 5900
…
  vncviewer 10.1.1.10:5900     ← interpreted as display 5900 by many clients
```

Diagnostic from aether: VNC IS listening on 10.1.1.10:5900, firewall is
open. The viewer command is the only thing wrong.

### Fix

`libvirt::vnc_display` already parses `virsh vncdisplay <name>` output
(`10.1.1.10:0`). Today it returns the **port** (5900 + display). Change
its return type to a struct with both `display` (u16) and `port` (u16),
or return `(display, port)`.

`vnc.rs::run` then prints:

```
VNC for 'Hermes':
  bind     : 10.1.1.10
  display  : :0
  port     : 5900

From a VNC viewer:
  vncviewer 10.1.1.10:0          # canonical: host:display
  vncviewer 10.1.1.10::5900      # explicit port form (always works)

From a browser via macOS Screen Sharing:
  open vnc://10.1.1.10           # connects to display 0 by default
```

Loopback bind keeps the SSH tunnel instructions; only the per-line strings
change.

`--open` already passes `host:port` to `vncviewer`, which is wrong for
the same reason — change to `host::port`.

### Test plan on aether

1. After deploying new binary, delete `Hermes` (user authorised).
2. Create a fresh `testvm` via TUI with default distro.
3. `qvm vnc testvm` → confirm the printed forms.
4. From this laptop: `open vnc://10.1.1.10` → macOS Screen Sharing should
   connect to display 0.
5. Bonus: temporarily run `websockify --web=/usr/share/novnc 6080
   10.1.1.10:5900` on aether and open `http://10.1.1.10:6080/vnc.html`
   in Chrome to verify the browser path. Tear down websockify after.

## Verification (full)

- `cargo test --release --locked` — all existing 33 tests pass plus new
  ones for `vnc_display` returning both fields and for `util::valid_vm_name`
  edge cases triggered by the TUI form.
- `cargo clippy --release --all-targets --locked -- -D warnings` — clean.
- `shellcheck install.sh` — unchanged (no shell changes in this round).
- Manual on aether: bare `qvm` opens TUI; create flow works; auto-pull
  triggers for an uninstalled distro; `e` opens console; VNC connect
  string works via `open vnc://`.

## Risk / rollback

- TUI breaks on weird terminals → user can always invoke any subcommand
  directly; the TUI is the *only* path that uses raw mode. No regression
  to CLI flows.
- Auto-pull silently downloading >1GB → mitigated by the explicit
  "pulling…" line and by `qemu-img convert -p` showing pull progress.
  User can Ctrl-C.
- ratatui binary growth → measured before merge; if >1MB increase,
  reconsider.

## Out-of-band notes captured during diagnosis (2026-05-21)

- aether: Ubuntu 26.04 LTS, libvirt 12.0, qemu 10.2.1, qvm 0.1.0.
- Only VM today: `Hermes`, running, no real workload — safe to delete
  for testing.
- LAN with no firewall between aether (10.1.1.10) and laptop. No need
  for SSH tunnels during tests.
