use std::{ffi::c_void, fs::File, io::Write, ptr, slice};

const USER_DATA: i32 = 1;
const WRITE_PTY: i32 = 2;
const OUT_OF_SPACE: i32 = -3;
pub(super) const COLUMNS: u16 = 100;
pub(super) const ROWS: u16 = 40;

// The native terminal already links the pinned Ghostty VT C library.
unsafe extern "C" {
    fn ghostty_terminal_new(
        allocator: *const c_void,
        terminal: *mut *mut c_void,
        columns: u16,
        rows: u16,
    ) -> i32;
    fn ghostty_terminal_free(terminal: *mut c_void);
    fn ghostty_terminal_set(terminal: *mut c_void, option: i32, data: *const c_void) -> i32;
    fn ghostty_terminal_vt_write(terminal: *mut c_void, bytes: *const u8, length: usize);
    fn ghostty_formatter_terminal_new(
        allocator: *const c_void,
        formatter: *mut *mut c_void,
        terminal: *mut c_void,
    ) -> i32;
    fn ghostty_formatter_format_buf(
        formatter: *mut c_void,
        bytes: *mut u8,
        length: usize,
        written: *mut usize,
    ) -> i32;
    fn ghostty_formatter_free(formatter: *mut c_void);
}

extern "C" fn reply(_: *mut c_void, private: *mut c_void, bytes: *const u8, length: usize) {
    // Ghostty calls this synchronously with the writer and bytes supplied below.
    let (writer, bytes) = unsafe {
        (
            &mut *private.cast::<File>(),
            slice::from_raw_parts(bytes, length),
        )
    };
    let _ = writer.write_all(bytes);
}

pub(super) struct Screen {
    handles: (*mut c_void, *mut c_void),
    _reply: Box<File>,
    rows: u16,
}

impl Screen {
    pub(super) fn new(writer: File, columns: u16, rows: u16) -> Self {
        let mut terminal = ptr::null_mut();
        let mut formatter = ptr::null_mut();
        let mut writer = Box::new(writer);
        unsafe {
            assert_eq!(
                ghostty_terminal_new(ptr::null(), &mut terminal, columns, rows),
                0
            );
            assert_eq!(
                ghostty_formatter_terminal_new(ptr::null(), &mut formatter, terminal),
                0
            );
            assert_eq!(
                ghostty_terminal_set(terminal, USER_DATA, ptr::from_mut(&mut *writer).cast()),
                0
            );
            assert_eq!(
                ghostty_terminal_set(terminal, WRITE_PTY, reply as *const c_void),
                0
            );
        }
        Self {
            handles: (terminal, formatter),
            _reply: writer,
            rows,
        }
    }

    pub(super) fn feed(&mut self, bytes: &[u8]) {
        unsafe { ghostty_terminal_vt_write(self.handles.0, bytes.as_ptr(), bytes.len()) }
    }

    pub(super) fn history_text(&self) -> String {
        let mut length = 0;
        let need = unsafe {
            ghostty_formatter_format_buf(self.handles.1, ptr::null_mut(), 0, &mut length)
        };
        if need == 0 {
            return String::new();
        }
        assert_eq!(need, OUT_OF_SPACE);
        assert!(length <= 1_048_576);
        let mut bytes = vec![0; length];
        let result = unsafe {
            ghostty_formatter_format_buf(
                self.handles.1,
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut length,
            )
        };
        assert_eq!(result, 0);
        bytes.truncate(length);
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub(super) fn visible_text(&self) -> String {
        let text = self.history_text();
        let mut lines = text
            .lines()
            .rev()
            .take(self.rows as usize)
            .collect::<Vec<_>>();
        lines.reverse();
        lines.join("\n")
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        unsafe {
            ghostty_formatter_free(self.handles.1);
            ghostty_terminal_free(self.handles.0);
        }
    }
}
