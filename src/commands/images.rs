use crate::config::Config;
use crate::error::Result;
use crate::style;

pub fn distros(cfg: &Config) -> Result<()> {
    println!(
        "  {:<14} {:<8} {:<7} {}",
        style::label("DISTRO"),
        style::label("FIRMWARE"),
        style::label("PULLED"),
        style::label("IMAGE"),
    );
    for (key, d) in &cfg.distros {
        let fw = if d.uefi { "uefi" } else { "bios" };
        let img = cfg.paths.images.join(&d.image);
        let pulled = img.exists();
        println!(
            "  {:<14} {:<8} {:<7} {}",
            key,
            fw,
            style::yes_no(pulled),
            style::dim(&d.image),
        );
    }
    Ok(())
}

pub fn images(cfg: &Config) -> Result<()> {
    println!(
        "  {:<40} {:<7} {}",
        style::label("IMAGE"),
        style::label("EXISTS"),
        style::label("SIZE"),
    );
    for d in cfg.distros.values() {
        let p = cfg.paths.images.join(&d.image);
        if p.exists() {
            let size = match std::fs::metadata(&p) {
                Ok(m) => human_bytes(m.len()),
                Err(_) => "?".into(),
            };
            println!("  {:<40} {:<7} {}", d.image, style::yes_no(true), size);
        } else {
            println!("  {:<40} {:<7} {}", d.image, style::yes_no(false), style::dim("-"));
        }
    }
    Ok(())
}

fn human_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B","K","M","G","T"];
    let mut v = n as f64; let mut i = 0;
    while v >= 1024.0 && i < U.len()-1 { v /= 1024.0; i += 1; }
    if i == 0 { format!("{n}B") } else { format!("{v:.1}{}", U[i]) }
}
