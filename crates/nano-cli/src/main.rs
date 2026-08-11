//! wayland-nano — Wayland Nano (Track B) binary: doctor, protocol host.

mod doctor;
mod host_mode;

use nano_cli::acp_mode;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let exit_code = match args.get(1).map(String::as_str) {
        Some("doctor") => {
            let nano_home = nano_home();
            let mut out = std::io::stdout();
            doctor::run(&nano_home, &mut out).unwrap_or(2)
        }
        Some("acp-host") => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            let home = nano_home();
            match runtime.block_on(acp_mode::run(&home)) {
                Ok(code) => code,
                Err(err) => {
                    eprintln!("wayland-nano: acp io error: {err}");
                    2
                }
            }
        }
        Some("protocol-host") => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            let home = nano_home();
            let workspace = std::env::current_dir().expect("cwd");
            match runtime.block_on(host_mode::run(&home, &workspace)) {
                Ok(host_mode::HostExit::StdinClosed) => 0,
                Ok(host_mode::HostExit::ShutdownCommand) => 0,
                Ok(host_mode::HostExit::Fatal(reason)) => {
                    eprintln!("wayland-nano: fatal: {reason}");
                    2
                }
                Err(err) => {
                    eprintln!("wayland-nano: host loop io error: {err}");
                    2
                }
            }
        }
        Some("--version") | Some("-V") => {
            println!("wayland-nano {}", env!("CARGO_PKG_VERSION"));
            0
        }
        _ => {
            eprintln!("usage: wayland-nano doctor | protocol-host | acp-host | --version");
            2
        }
    };
    std::process::exit(exit_code);
}

fn nano_home() -> std::path::PathBuf {
    std::env::var_os("NANO_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            dirs_next::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".nano")
        })
}
