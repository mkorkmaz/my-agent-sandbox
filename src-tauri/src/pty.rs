use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

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
