use std::fs::OpenOptions;
use std::io::Write;

fn main() {
    let path = std::env::var_os("NANO_ARGV_ECHO_LOG").expect("NANO_ARGV_ECHO_LOG");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open argv log");
    writeln!(log, "{}", args.join("\u{1f}")).expect("append argv log");
    log.sync_data().expect("sync argv log");
    if args.first().is_some_and(|arg| arg == "fail") {
        std::process::exit(1);
    }
}
