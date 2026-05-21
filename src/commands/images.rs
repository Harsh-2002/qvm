use crate::config::Config;
use crate::error::{Error, Result};
use crate::style;
use serde::Serialize;

#[derive(Debug, Serialize)]
struct DistroRow {
    name:     String,
    arch:     String,
    image:    String,
    osinfo:   String,
    firmware: &'static str,
    pulled:   bool,
}

#[derive(Debug, Serialize)]
struct ImageRow {
    image:  String,
    exists: bool,
    bytes:  Option<u64>,
}

pub fn distros(cfg: &Config, json: bool) -> Result<()> {
    let host = crate::arch::host();
    let mut rows: Vec<DistroRow> = Vec::new();
    for (key, d) in &cfg.distros {
        let (image, _) = match d.variant_for(host) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let pulled = cfg.paths.images.join(image).exists();
        let firmware = if d.uefi || crate::arch::is_arm() { "uefi" } else { "bios" };
        rows.push(DistroRow {
            name: key.clone(),
            arch: host.to_string(),
            image: image.to_string(),
            osinfo: d.osinfo.clone(),
            firmware,
            pulled,
        });
    }
    if json {
        return print_json(&rows);
    }
    println!(
        "  {:<14} {:<8} {:<7} {}",
        style::label("DISTRO"),
        style::label("FIRMWARE"),
        style::label("PULLED"),
        style::label("IMAGE"),
    );
    for r in &rows {
        println!(
            "  {:<14} {:<8} {:<7} {}",
            r.name,
            r.firmware,
            style::yes_no(r.pulled),
            style::dim(&r.image),
        );
    }
    Ok(())
}

pub fn images(cfg: &Config, json: bool) -> Result<()> {
    let host = crate::arch::host();
    let mut rows: Vec<ImageRow> = Vec::new();
    for d in cfg.distros.values() {
        let image = match d.variant_for(host) {
            Ok((img, _)) => img,
            Err(_) => continue,
        };
        let p = cfg.paths.images.join(image);
        let (exists, bytes) = match std::fs::metadata(&p) {
            Ok(m) if m.is_file() => (true, Some(m.len())),
            _                    => (false, None),
        };
        rows.push(ImageRow { image: image.to_string(), exists, bytes });
    }
    if json {
        return print_json(&rows);
    }
    println!(
        "  {:<40} {:<7} {}",
        style::label("IMAGE"),
        style::label("EXISTS"),
        style::label("SIZE"),
    );
    for r in &rows {
        let size = match r.bytes {
            Some(b) => human_bytes(b),
            None    => style::dim("-").to_string(),
        };
        println!("  {:<40} {:<7} {}", r.image, style::yes_no(r.exists), size);
    }
    Ok(())
}

fn print_json<T: Serialize>(rows: &T) -> Result<()> {
    let s = serde_json::to_string_pretty(rows)
        .map_err(|e| Error::User(format!("json encode: {e}")))?;
    println!("{s}");
    Ok(())
}

fn human_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B","K","M","G","T"];
    let mut v = n as f64; let mut i = 0;
    while v >= 1024.0 && i < U.len()-1 { v /= 1024.0; i += 1; }
    if i == 0 { format!("{n}B") } else { format!("{v:.1}{}", U[i]) }
}
