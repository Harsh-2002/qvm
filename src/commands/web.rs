//! `qvm web` — launch the embedded HTTP UI in the foreground.

use crate::config::Config;
use crate::error::Result;

pub fn run(cfg: Config, bind: String, port: u16) -> Result<()> {
    crate::web::run(cfg, &bind, port)
}
