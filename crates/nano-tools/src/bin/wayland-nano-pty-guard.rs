//! `wayland-nano-pty-guard` — unix PTY host-death sentinel.
//!
//! portable-pty 0.9 `setsid()`s the PTY child into its own session and exposes
//! no pre-exec hook, so a host `kill -9` would leave the session leader (and
//! its tree) orphaned (design note §14 leg 3(b)). The guard is spawned AS the
//! PTY child in place of the real command: after `setsid()` it is the
//! process-group leader, so its group holds the whole PTY tree (the sandbox
//! helper / `sandbox-exec` / shell chain it execs stays in the group).
//!
//! The guard supervises instead of exec'ing: it watches its parent (the Nano
//! host) for death — `pidfd_open` + `poll` on Linux, kqueue `EVFILT_PROC` /
//! `NOTE_EXIT` on macOS, mirroring the parent-death binding the repo's Linux
//! sandbox helper already applies to its bubblewrap child — and on parent exit
//! SIGKILLs its entire process group. Watcher setup failure, a poll/kevent
//! error, or an already-dead parent all fail closed: the group is killed
//! rather than left unwatched. Otherwise the guard waits on the wrapped
//! command and propagates its exit status.

#[cfg(unix)]
mod imp {
    use std::ffi::OsString;
    use std::process::ExitCode;

    pub fn run() -> ExitCode {
        let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
        let Some((program, args)) = argv.split_first() else {
            eprintln!("wayland-nano-pty-guard: missing wrapped command");
            return ExitCode::from(64);
        };
        let parent = unsafe { libc::getppid() };
        if parent == 1 {
            // Reparented to init/launchd before we could bind: the host died
            // between portable-pty's fork and our exec. Fail closed.
            kill_group_and_exit();
        }
        let watcher = ParentWatcher::bind(parent);
        // Re-check after binding (the repo's terminate_with_parent
        // discipline): if the host died mid-bind the watcher may hold a
        // reused pid — never run the command unwatched.
        if unsafe { libc::getppid() } != parent {
            kill_group_and_exit();
        }
        std::thread::spawn(move || watcher.wait());
        match std::process::Command::new(program).args(args).status() {
            Ok(status) => {
                use std::os::unix::process::ExitStatusExt;
                let code = status
                    .code()
                    .unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
                ExitCode::from(code as u8)
            }
            Err(error) => {
                eprintln!(
                    "wayland-nano-pty-guard: failed to spawn {}: {error}",
                    program.to_string_lossy()
                );
                ExitCode::from(71)
            }
        }
    }

    /// A bound parent-death watch: the OS watch is registered in `bind`
    /// (before the wrapped command starts) so `run` can re-verify the parent
    /// afterwards; `wait` blocks until the parent exits and then kills the
    /// whole process group. Every bind/wait failure fails closed.
    #[cfg(target_os = "linux")]
    struct ParentWatcher {
        descriptor: libc::c_int,
    }

    #[cfg(target_os = "linux")]
    impl ParentWatcher {
        fn bind(parent: libc::pid_t) -> Self {
            // libc exposes no glibc `pidfd_open` wrapper for gnu targets; the
            // syscall number is stable (kernel 5.3+).
            let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, parent, 0) };
            if descriptor < 0 {
                // ESRCH: parent already dead. Anything else (e.g. ENOSYS on a
                // pre-5.3 kernel): the parent cannot be watched — fail closed.
                kill_group_and_exit();
            }
            Self {
                descriptor: descriptor as libc::c_int,
            }
        }

        fn wait(self) {
            let mut polled = libc::pollfd {
                fd: self.descriptor,
                events: libc::POLLIN,
                revents: 0,
            };
            loop {
                let result = unsafe { libc::poll(&mut polled, 1, -1) };
                if result > 0 {
                    kill_group_and_exit();
                }
                if result < 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
                {
                    kill_group_and_exit();
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    struct ParentWatcher {
        queue: libc::c_int,
    }

    #[cfg(target_os = "macos")]
    impl ParentWatcher {
        fn bind(parent: libc::pid_t) -> Self {
            let queue = unsafe { libc::kqueue() };
            if queue < 0 {
                kill_group_and_exit();
            }
            let registration = libc::kevent {
                ident: parent as usize,
                filter: libc::EVFILT_PROC,
                flags: libc::EV_ADD | libc::EV_ONESHOT,
                fflags: libc::NOTE_EXIT,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            let registered = unsafe {
                libc::kevent(
                    queue,
                    &registration,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            };
            if registered < 0 {
                // ESRCH: parent already dead. Any other error leaves the
                // parent unwatched — fail closed either way.
                kill_group_and_exit();
            }
            Self { queue }
        }

        fn wait(self) {
            let mut event: libc::kevent = unsafe { std::mem::zeroed() };
            loop {
                let count = unsafe {
                    libc::kevent(
                        self.queue,
                        std::ptr::null(),
                        0,
                        &mut event,
                        1,
                        std::ptr::null(),
                    )
                };
                if count > 0 {
                    kill_group_and_exit();
                }
                if count < 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
                {
                    kill_group_and_exit();
                }
            }
        }
    }

    /// SIGKILL every process in the guard's own process group (signal pid 0).
    /// The guard is the group leader post-`setsid()`, so this takes down the
    /// entire PTY tree, the guard included.
    fn kill_group_and_exit() -> ! {
        unsafe { libc::kill(0, libc::SIGKILL) };
        unsafe { libc::_exit(1) }
    }
}

#[cfg(unix)]
fn main() -> std::process::ExitCode {
    imp::run()
}

#[cfg(not(unix))]
fn main() {
    panic!("wayland-nano-pty-guard is unix-only");
}
