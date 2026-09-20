#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{env, path, process};

use anyhow::Context;

const HELP: &str = "ComPort: native Mattermost client\n\
\n\
Usage: comport [--server URL]\n\
       comport --demo [--snapshot PNG]\n\
       comport --version\n\
       comport --help\n\
\n\
With no flags, opens a login form for your Mattermost Site URL.\n\
--server prefills that URL (https://chat.company.com).\n\
--demo loads recorded fixtures (no live server).\n\
--snapshot writes an offscreen PNG of the demo and requires --demo.\n";

fn main() -> process::ExitCode {
    match run() {
        Ok(()) => process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("comport: {error:#}");
            process::ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let mut arguments = env::args_os().skip(1);
    let mut demo = false;
    let mut snapshot = None;
    let mut server = None;
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--help" | "-h") => {
                print!("{HELP}");
                return Ok(());
            }
            Some("--version") => {
                let revision = comport::REVISION.unwrap_or("unknown");
                println!("ComPort {} ({revision})", comport::VERSION);
                return Ok(());
            }
            Some("--demo") => {
                anyhow::ensure!(!demo, "--demo supplied twice");
                demo = true;
            }
            Some("--server") => {
                anyhow::ensure!(server.is_none(), "--server supplied twice");
                server = Some(
                    arguments
                        .next()
                        .context("--server needs a Site URL")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            Some("--snapshot") => {
                anyhow::ensure!(snapshot.is_none(), "--snapshot supplied twice");
                snapshot = Some(path::PathBuf::from(
                    arguments.next().context("--snapshot needs a PNG path")?,
                ));
            }
            _ => anyhow::bail!("unrecognized argument {argument:?}; use --help"),
        }
    }
    anyhow::ensure!(
        server.is_none() || !demo,
        "--server cannot be combined with --demo"
    );
    let _ = env_logger::try_init();
    if let Some(path) = snapshot {
        anyhow::ensure!(demo, "--snapshot requires --demo");
        return comport::snapshot::save_demo(&path);
    }
    comport::window::run(comport::window::Startup { demo, site: server })
}
