//! A connection-local fault in the real SQLite database sync method.
use rusqlite::{ffi, Connection};
use std::sync::atomic::{AtomicBool, Ordering};

#[repr(C)]
struct Methods {
    methods: ffi::sqlite3_io_methods,
    original: *const ffi::sqlite3_io_methods,
    failed: AtomicBool,
}

pub(super) struct SyncFault {
    file: *mut ffi::sqlite3_file,
    methods: Box<Methods>,
}

unsafe extern "C" fn fail_once(file: *mut ffi::sqlite3_file, flags: i32) -> i32 {
    let methods = unsafe { &*((*file).pMethods.cast::<Methods>()) };
    if !methods.failed.swap(true, Ordering::SeqCst) {
        // Restore before returning: the caller may invalidate this connection on error.
        unsafe {
            (*file).pMethods = methods.original;
        }
        return ffi::SQLITE_IOERR_FSYNC;
    }
    unsafe { ((*methods.original).xSync.unwrap())(file, flags) }
}

impl SyncFault {
    pub(super) unsafe fn install(connection: &Connection) -> Self {
        let mut file = std::ptr::null_mut::<ffi::sqlite3_file>();
        let result = unsafe {
            ffi::sqlite3_file_control(
                connection.handle(),
                c"main".as_ptr(),
                ffi::SQLITE_FCNTL_FILE_POINTER,
                (&mut file as *mut *mut ffi::sqlite3_file).cast(),
            )
        };
        assert_eq!(result, ffi::SQLITE_OK);
        assert!(!file.is_null());
        let original = unsafe { (*file).pMethods };
        let mut methods = Box::new(Methods {
            methods: unsafe { *original },
            original,
            failed: AtomicBool::new(false),
        });
        methods.methods.xSync = Some(fail_once);
        unsafe {
            (*file).pMethods = &methods.methods;
        }
        Self { file, methods }
    }
    pub(super) fn failed(&self) -> bool {
        self.methods.failed.load(Ordering::SeqCst)
    }
}

impl Drop for SyncFault {
    fn drop(&mut self) {
        if self.failed() {
            return;
        }
        unsafe {
            (*self.file).pMethods = self.methods.original;
        }
    }
}
