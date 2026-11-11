use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// Starting and stopping share a gate so a cancelled, reserved terminal cannot fork later.
static START_STOP: Mutex<()> = Mutex::new(());

pub struct Process {
    pid: AtomicI32,
    running: AtomicBool,
    closing: AtomicBool,
    control: Mutex<File>,
}

impl Process {
    pub fn new(pty: &Pty) -> io::Result<Self> {
        Ok(Self {
            pid: AtomicI32::new(0),
            running: AtomicBool::new(true),
            closing: AtomicBool::new(false),
            control: Mutex::new(pty.master.try_clone()?),
        })
    }

    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn spawn(&self, pty: &Pty, command: &mut Command) -> io::Result<Child> {
        let _guard = START_STOP.lock().unwrap_or_else(|error| error.into_inner());
        if self.closing.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Terminal start was cancelled.",
            ));
        }
        let child = pty.spawn(command)?;
        self.pid.store(child.id() as i32, Ordering::SeqCst);
        Ok(child)
    }

    fn signal(&self, signal: i32) {
        let pid = self.pid.load(Ordering::SeqCst);
        if pid <= 0 {
            return;
        }
        if let Ok(control) = self.control.lock() {
            let foreground = unsafe { libc::tcgetpgrp(control.as_raw_fd()) };
            if foreground > 0 && foreground != pid {
                unsafe {
                    libc::kill(-foreground, signal);
                }
            }
        }
        unsafe {
            libc::kill(-pid, signal);
        }
    }

    pub fn stop(&self, signal: i32) {
        let _guard = START_STOP.lock().unwrap_or_else(|error| error.into_inner());
        self.closing.store(true, Ordering::SeqCst);
        self.signal(signal);
        if self.pid.load(Ordering::SeqCst) == 0 {
            self.running.store(false, Ordering::SeqCst);
        }
    }

    pub fn exited(&self) {
        let _guard = START_STOP.lock().unwrap_or_else(|error| error.into_inner());
        // A shell can exit while its foreground job still owns the terminal.
        self.signal(libc::SIGKILL);
        self.pid.store(0, Ordering::SeqCst);
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn resize(&self, columns: u16, rows: u16, width: u16, height: u16) {
        if let Ok(control) = self.control.lock() {
            let _ = resize(&control, columns, rows, width, height);
        }
    }
}

pub fn terminate(processes: &[Arc<Process>], grace: Duration) -> Result<(), String> {
    for process in processes {
        process.stop(libc::SIGHUP);
    }
    let started = Instant::now();
    while processes.iter().any(|process| process.running()) && started.elapsed() < grace {
        std::thread::sleep(Duration::from_millis(10));
    }
    for process in processes.iter().filter(|process| process.running()) {
        process.stop(libc::SIGKILL);
    }
    let started = Instant::now();
    while processes.iter().any(|process| process.running())
        && started.elapsed() < Duration::from_secs(3)
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    if processes.iter().any(|process| process.running()) {
        return Err(
            "A terminal process has not exited. The app is still open; retry closing it.".into(),
        );
    }
    Ok(())
}

pub struct Pty {
    pub master: File,
    slave: File,
}

impl Pty {
    pub fn open() -> io::Result<Self> {
        let mut master = -1;
        let mut slave = -1;
        let mut size = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // openpty returns two owned descriptors. The File values close them on all paths.
        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if result == -1 {
            return Err(io::Error::last_os_error());
        }
        for descriptor in [master, slave] {
            if unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
                let error = io::Error::last_os_error();
                unsafe {
                    libc::close(master);
                    libc::close(slave);
                }
                return Err(error);
            }
        }
        Ok(Self {
            master: unsafe { File::from_raw_fd(master) },
            slave: unsafe { File::from_raw_fd(slave) },
        })
    }

    pub fn spawn(&self, command: &mut Command) -> io::Result<Child> {
        command.stdin(Stdio::from(self.slave.try_clone()?));
        command.stdout(Stdio::from(self.slave.try_clone()?));
        command.stderr(Stdio::from(self.slave.try_clone()?));
        // Only async-signal-safe syscalls run between fork and exec.
        unsafe {
            command.pre_exec(|| {
                for signal in [
                    libc::SIGINT,
                    libc::SIGQUIT,
                    libc::SIGHUP,
                    libc::SIGTERM,
                    libc::SIGTSTP,
                ] {
                    libc::signal(signal, libc::SIG_DFL);
                }
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc::tcsetpgrp(0, libc::getpgrp()) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command.spawn()
    }
}

pub fn resize(master: &File, columns: u16, rows: u16, width: u16, height: u16) -> io::Result<()> {
    let size = libc::winsize {
        ws_row: rows,
        ws_col: columns,
        ws_xpixel: width,
        ws_ypixel: height,
    };
    if unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &size) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn shutdown_waits_for_final_writes_and_kills_stubborn_processes() {
        for stubborn in [false, true] {
            let pty = Pty::open().unwrap();
            let process = Arc::new(Process::new(&pty).unwrap());
            let path = std::env::temp_dir().join(format!(
                "tugboat-pty-exit-{}-{stubborn}",
                std::process::id()
            ));
            let mut command = Command::new("/bin/sh");
            let script = if stubborn {
                "trap '' HUP; printf ready; while :; do sleep 1; done"
            } else {
                "trap 'printf final > \"$1\"; exit 0' HUP; printf ready; while :; do read line; done"
            };
            command.args(["-c", script, "fixture"]).arg(&path);
            let mut child = process.spawn(&pty, &mut command).unwrap();
            let mut reader = pty.master.try_clone().unwrap();
            let mut ready = [0; 5];
            reader.read_exact(&mut ready).unwrap();
            drop(pty);
            let drain = std::thread::spawn(move || {
                let _ = reader.read_to_end(&mut Vec::new());
            });
            let waiting = process.clone();
            let waiter = std::thread::spawn(move || {
                let status = child.wait().unwrap();
                waiting.exited();
                status
            });
            terminate(std::slice::from_ref(&process), Duration::from_millis(70)).unwrap();
            assert!(!process.running());
            let status = waiter.join().unwrap();
            if stubborn {
                assert_eq!(status.signal(), Some(libc::SIGKILL));
            } else {
                assert_eq!(std::fs::read_to_string(&path).unwrap(), "final");
                std::fs::remove_file(&path).unwrap();
            }
            drain.join().unwrap();
        }
    }

    #[test]
    fn stopped_reservation_cannot_start_a_process() {
        let pty = Pty::open().unwrap();
        let process = Arc::new(Process::new(&pty).unwrap());
        terminate(std::slice::from_ref(&process), Duration::ZERO).unwrap();
        assert_eq!(
            process
                .spawn(&pty, &mut Command::new("/bin/true"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::Interrupted
        );
        assert!(!process.running());
    }

    #[test]
    fn child_has_a_controlling_terminal_and_shared_stdio() {
        let pty = Pty::open().unwrap();
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "test -t 0 && test -t 1 && test -t 2 && tty && stty size",
        ]);
        resize(&pty.master, 97, 31, 970, 620).unwrap();
        let mut child = pty.spawn(&mut command).unwrap();
        let mut master = pty.master.try_clone().unwrap();
        drop(pty);
        let mut output = String::new();
        let result = master.read_to_string(&mut output);
        assert!(result.is_ok() || result.unwrap_err().raw_os_error() == Some(libc::EIO));
        assert!(child.wait().unwrap().success());
        assert!(output.contains("/dev/ttys"), "{output}");
        assert!(output.contains("31 97"), "{output}");
    }

    #[test]
    fn keyboard_interrupt_reaches_foreground_process() {
        let pty = Pty::open().unwrap();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf ready; exec /bin/sleep 5"]);
        let mut child = pty.spawn(&mut command).unwrap();
        let mut master = pty.master.try_clone().unwrap();
        drop(pty);
        let mut ready = [0; 5];
        master.read_exact(&mut ready).unwrap();
        assert_eq!(&ready, b"ready");
        master.write_all(&[3]).unwrap();
        // Darwin drains terminal output during exit. Keep reading while the child exits.
        let mut tail = Vec::new();
        let result = master.read_to_end(&mut tail);
        assert!(result.is_ok() || result.unwrap_err().raw_os_error() == Some(libc::EIO));
        assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGINT));
    }

    #[test]
    fn resizing_delivers_sigwinch_and_the_new_cell_dimensions() {
        let pty = Pty::open().unwrap();
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "trap 'stty size; exit 0' WINCH; printf ready; read line",
        ]);
        let mut child = pty.spawn(&mut command).unwrap();
        let mut master = pty.master.try_clone().unwrap();
        let mut ready = [0; 5];
        master.read_exact(&mut ready).unwrap();
        assert_eq!(&ready, b"ready");
        resize(&pty.master, 101, 37, 1010, 740).unwrap();
        master.write_all(b"\n").unwrap();
        drop(pty);
        let mut output = String::new();
        let result = master.read_to_string(&mut output);
        assert!(result.is_ok() || result.unwrap_err().raw_os_error() == Some(libc::EIO));
        assert!(child.wait().unwrap().success());
        assert!(output.contains("37 101"), "{output}");
    }
}
