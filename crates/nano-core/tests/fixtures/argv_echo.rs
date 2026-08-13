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
    // ONE write syscall: write_fmt can split into multiple writes, and a
    // piped sibling's writes then interleave mid-line (CI-proven tear on
    // ubuntu-22.04: "producerconsumer"). A single short write under
    // O_APPEND is atomic on POSIX.
    let line = format!("{}\n", args.join("\u{1f}"));
    log.write_all(line.as_bytes()).expect("append argv log");
    log.sync_data().expect("sync argv log");
    if args.first().is_some_and(|arg| arg == "fail") {
        std::process::exit(1);
    }
}
