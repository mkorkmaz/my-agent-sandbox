use serde::{de::DeserializeOwned, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_WRITE: AtomicU64 = AtomicU64::new(1);

pub fn read<T: DeserializeOwned>(file: &Path) -> Result<Option<T>, String> {
    let bytes = match fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "Could not read the saved settings. Check the file.".into())
}

pub fn write(file: &Path, value: &impl Serialize) -> Result<(), String> {
    let directory = file.parent().ok_or("Invalid storage location.")?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let temporary = file.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
        output
            .write_all(&bytes)
            .map_err(|error| error.to_string())?;
        output.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&temporary, file).map_err(|error| error.to_string())?;
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
