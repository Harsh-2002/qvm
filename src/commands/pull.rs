use crate::config::Config;
use crate::error::{Error, Result};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

pub fn run(cfg: &Config, distro: &str) -> Result<()> {
    let d = cfg.distro(distro)?;
    println!("Pulling {distro}:");
    println!("  {}", d.url);
    println!("  -> {}", cfg.image_path(distro)?.display());
    pull_one(cfg, distro)?;
    println!("Ready: {}", cfg.image_path(distro)?.display());
    Ok(())
}

/// Atomic download for a single distro: stream into `<image>.partial`,
/// rename on success. No external `wget` dependency — uses an embedded
/// HTTPS client (`ureq` + rustls + bundled Mozilla CA roots).
pub fn pull_one(cfg: &Config, distro: &str) -> Result<()> {
    let d    = cfg.distro(distro)?;
    let dest = cfg.image_path(distro)?;
    let tmp  = dest.with_extension("partial");

    // Make sure the parent dir exists — caller is supposed to have run
    // ensure_dirs() but be defensive.
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::remove_file(&tmp);

    if let Err(e) = download_with_progress(&d.url, &tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(Error::User(format!(
            "download failed for {distro}: {e}; {} is unchanged.",
            dest.display()
        )));
    }
    fs::rename(&tmp, &dest)?;
    Ok(())
}

/// HTTP GET + stream to disk with a single-line progress indicator.
fn download_with_progress(url: &str, dest: &Path) -> std::result::Result<(), String> {
    let resp = ureq::get(url).call()
        .map_err(|e| format!("HTTP request: {e}"))?;
    if resp.status() != 200 {
        return Err(format!("HTTP {} from server", resp.status()));
    }
    let total: u64 = resp.headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let mut file = fs::File::create(dest)
        .map_err(|e| format!("create {}: {e}", dest.display()))?;
    let mut body = resp.into_body().into_reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut downloaded: u64 = 0;
    let mut last_paint = Instant::now() - Duration::from_secs(1);
    let started = Instant::now();

    loop {
        let n = body.read(&mut buf).map_err(|e| format!("read: {e}"))?;
        if n == 0 { break; }
        file.write_all(&buf[..n]).map_err(|e| format!("write: {e}"))?;
        downloaded += n as u64;
        if last_paint.elapsed() >= Duration::from_millis(120) {
            paint_progress(downloaded, total, started.elapsed());
            last_paint = Instant::now();
        }
    }
    paint_progress(downloaded, total, started.elapsed());
    eprintln!();
    file.sync_all().map_err(|e| format!("sync: {e}"))?;
    Ok(())
}

/// Single-line progress drawn to stderr with `\r` so subsequent paints
/// overwrite. Falls back to a counter-only display if Content-Length was
/// missing.
fn paint_progress(done: u64, total: u64, elapsed: Duration) {
    let mb = (done as f64) / 1024.0 / 1024.0;
    let speed = if elapsed.as_secs_f64() > 0.0 {
        mb / elapsed.as_secs_f64()
    } else { 0.0 };

    if total > 0 {
        let pct = ((done as f64 / total as f64) * 100.0).min(100.0);
        let total_mb = (total as f64) / 1024.0 / 1024.0;
        // 30-wide bar: `[█████···                    ]`
        let bar_w = 30usize;
        let filled = ((pct / 100.0) * bar_w as f64).round() as usize;
        let filled = filled.min(bar_w);
        let mut bar = String::with_capacity(bar_w);
        for _ in 0..filled        { bar.push('█'); }
        for _ in 0..(bar_w - filled) { bar.push(' '); }
        eprint!("\r  [{bar}] {pct:>5.1}%  {mb:>6.1}/{total_mb:.1} MB  {speed:>5.1} MB/s");
    } else {
        eprint!("\r  {mb:>6.1} MB  {speed:.1} MB/s");
    }
    let _ = std::io::stderr().flush();
}
