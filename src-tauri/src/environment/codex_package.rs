use crate::{
    platform::{brig::Brig, command, runtime},
    storage,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, DirBuilder, OpenOptions},
    io::{self, Read},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

pub(super) const VERSION: &str = "codex-cli 0.171.0";
pub(crate) const PROGRAM: &str = "/root/.tugboat-codex-0.171.0/bin/codex";
const DIRECTORY: &str = ".tugboat-codex-0.171.0";
const RELEASE: &str = "https://github.com/openai/codex/releases/download/rust-v0.171.0";
const SIGNER: &str =
    "https://github.com/openai/codex/.github/workflows/rust-release.yml@refs/tags/rust-v0.171.0";
const ISSUER: &str = "https://token.actions.githubusercontent.com";
const ARCHIVE: &str = "codex-package-aarch64-unknown-linux-musl.tar.gz";
const BUNDLE: &str = "codex-package-aarch64-unknown-linux-musl.tar.gz.sigstore";
const ARCHIVE_SIZE: u64 = 199_315_347;
const BUNDLE_SIZE: u64 = 8_489;
const ARCHIVE_SHA256: &str = "d8c43bee6cf11b01eb18b95f4d7961092bc40228180808859e70dabb7784c1c9";
const BUNDLE_SHA256: &str = "ad7a3d4d1703a6c664c222d77c462a3932072bba5dfced64677d148cbbe640ff";
const MANIFEST: &str = include_str!("../../profiles/codex-0.171.0.manifest");

pub(super) fn install(brig: &Brig, home: &Path, running: bool) -> Result<(), String> {
    if verify(home).is_ok() {
        return Ok(());
    }
    if running {
        return Err(
            "Stop the project guest before installing or repairing its Codex package.".into(),
        );
    }
    private_directory(home.parent().ok_or("The project guest home is invalid.")?)?;
    private_directory(home)?;
    let cache = brig.state_directory()?.join("tugboat-codex-0.171.0");
    private_directory(&cache)?;
    let _guard = storage::lock(&cache.join("package"))?;
    // A second preparation may have completed while this attempt waited for the cache.
    if verify(home).is_ok() {
        return Ok(());
    }
    let target = home.join(DIRECTORY);
    let existing_package = owned_tree(&target)?;
    let sources = cache.join("verified");
    let archive = sources.join(ARCHIVE);
    let bundle = sources.join(BUNDLE);
    if verify_archive(&archive, &bundle).is_err() {
        let existing_cache = owned_tree(&sources)?;
        let staged = Staging::new(&cache)?;
        download(brig, &staged.0, ARCHIVE, ARCHIVE_SIZE)?;
        download(brig, &staged.0, BUNDLE, BUNDLE_SIZE)?;
        verify_archive(&staged.0.join(ARCHIVE), &staged.0.join(BUNDLE))?;
        verify_signature(brig, &staged.0.join(ARCHIVE), &staged.0.join(BUNDLE))?;
        publish_directory(&staged.0, &sources, existing_cache)?;
    }
    check_directory(&sources, true)?;
    verify_archive(&archive, &bundle)?;
    let staged = Staging::new(home)?;
    let unpack = staged.0.join(DIRECTORY);
    private_directory(&unpack)?;
    run(
        brig,
        Path::new("/usr/bin/tar"),
        &[
            "-xzf",
            path(&archive)?,
            "--no-same-owner",
            "-C",
            path(&unpack)?,
        ],
        120,
        "Could not unpack the verified Codex guest package.",
    )?;
    verify(&staged.0)?;
    publish_directory(&unpack, &target, existing_package)?;
    Ok(())
}

fn owned_tree(directory: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
        Ok(_) => check_directory(directory, true)?,
    }
    fn check_entries(directory: &Path) -> Result<(), String> {
        for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            if metadata.uid() != unsafe { libc::geteuid() }
                || !(metadata.is_dir() || (metadata.is_file() && metadata.nlink() == 1))
            {
                return Err(
                    "The Codex package repair found a link or a file with unexpected ownership."
                        .into(),
                );
            }
            if metadata.is_dir() {
                check_entries(&entry.path())?;
            }
        }
        Ok(())
    }
    check_entries(directory)?;
    Ok(true)
}

#[cfg(target_os = "macos")]
fn publish_directory(staged: &Path, target: &Path, existing: bool) -> Result<(), String> {
    let source =
        std::ffi::CString::new(staged.as_os_str().as_bytes()).map_err(|error| error.to_string())?;
    let destination =
        std::ffi::CString::new(target.as_os_str().as_bytes()).map_err(|error| error.to_string())?;
    let flags = if existing {
        libc::RENAME_SWAP
    } else {
        libc::RENAME_EXCL
    };
    if unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), flags) } != 0 {
        return Err(format!(
            "Could not install the verified Codex package: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn publish_directory(_staged: &Path, _target: &Path, _existing: bool) -> Result<(), String> {
    Err("The shared project guest runtime requires macOS.".into())
}

pub(crate) fn verify(home: &Path) -> Result<(), String> {
    check_directory(home, true)?;
    let package = home.join(DIRECTORY);
    check_directory(&package, true)?;
    let mut pending = MANIFEST
        .lines()
        .map(|line| {
            let mut fields = line.splitn(4, ' ');
            let digest = fields.next().unwrap();
            let size = fields.next().unwrap().parse::<u64>().unwrap();
            let executable = fields.next().unwrap() == "1";
            (
                Path::new(fields.next().unwrap()),
                (digest, size, executable),
            )
        })
        .collect::<BTreeMap<_, _>>();
    verify_directory(&package, &package, &mut pending)?;
    if !pending.is_empty() {
        return Err(
            "The verified Codex guest package is incomplete. Prepare the project again.".into(),
        );
    }
    Ok(())
}

fn verify_directory<'a>(
    root: &Path,
    directory: &Path,
    pending: &mut BTreeMap<&'a Path, (&'a str, u64, bool)>,
) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "Invalid Codex package directory.")?;
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            if !pending.keys().any(|file| file.starts_with(relative)) {
                return Err("The Codex guest package contains an unexpected directory.".into());
            }
            check_directory(&path, false)?;
            verify_directory(root, &path, pending)?;
            continue;
        }
        let (digest, size, executable) = pending
            .remove(relative)
            .ok_or("The Codex guest package contains an unexpected file.")?;
        verify_file(&path, digest, size, executable)?;
    }
    Ok(())
}

fn verify_archive(archive: &Path, bundle: &Path) -> Result<(), String> {
    verify_file(archive, ARCHIVE_SHA256, ARCHIVE_SIZE, false)?;
    verify_file(bundle, BUNDLE_SHA256, BUNDLE_SIZE, false)
}

fn verify_signature(brig: &Brig, archive: &Path, bundle: &Path) -> Result<(), String> {
    let cosign = runtime::tool("cosign").ok_or("The bundled cosign executable is unavailable.")?;
    run(
        brig,
        &cosign,
        &[
            "verify-blob",
            "--bundle",
            path(bundle)?,
            "--certificate-identity",
            SIGNER,
            "--certificate-oidc-issuer",
            ISSUER,
            path(archive)?,
        ],
        90,
        "Could not verify the official Codex guest package signature.",
    )
}

fn download(brig: &Brig, directory: &Path, name: &str, size: u64) -> Result<(), String> {
    run(brig, Path::new("/usr/bin/curl"), &[
        "-qfsSL", "--proto", "=https", "--proto-redir", "=https", "--connect-timeout", "10",
        "--max-time", "360", "--max-filesize", &size.to_string(), "--output",
        path(&directory.join(name))?, &format!("{RELEASE}/{name}"),
    ], 370, "Could not download the official Codex guest package. Check the Mac's network connection and retry.")
}

fn run(
    brig: &Brig,
    executable: &Path,
    args: &[&str],
    timeout: u64,
    failure: &str,
) -> Result<(), String> {
    let arguments = args
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    let output = command::run(
        executable,
        &arguments,
        None,
        &brig.environment,
        Duration::from_secs(timeout),
        &AtomicBool::new(false),
    )
    .map_err(|_| failure)?;
    if output.exit_code != Some(0) || output.cancelled || output.timed_out || output.output_exceeded
    {
        return Err(failure.into());
    }
    Ok(())
}

fn path(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "The Codex guest package path is invalid.".into())
}

fn verify_file(path: &Path, expected: &str, size: u64, executable: bool) -> Result<(), String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let error = || format!("Could not verify the Codex guest package file {name}.");
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| error())?;
    let metadata = file.metadata().map_err(|_| error())?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.len() != size
        || (metadata.mode() & 0o111 != 0) != executable
    {
        return Err(error());
    }
    let mut digest = Sha256::new();
    let mut source = file.take(size + 1);
    let mut buffer = [0; 65_536];
    loop {
        let read = source.read(&mut buffer).map_err(|_| error())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    if source.limit() != 1 || crate::hash::hex(digest.finalize()) != expected {
        return Err(error());
    }
    Ok(())
}

fn private_directory(path: &Path) -> Result<(), String> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.to_string()),
    }
    check_directory(path, true)
}

fn check_directory(path: &Path, private: bool) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        "The Codex guest package directory is unavailable. Prepare the project again."
    })?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || (private
            && (metadata.mode() & 0o077 != 0
                || fs::canonicalize(path).ok().as_deref() != Some(path)))
    {
        return Err("The Codex guest package requires an owned, verified directory.".into());
    }
    Ok(())
}

struct Staging(PathBuf);

impl Staging {
    fn new(parent: &Path) -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let next = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".codex-staging-{}-{next}", std::process::id()));
        DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|error| error.to_string())?;
        Ok(Self(path))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            if error.kind() != io::ErrorKind::NotFound {
                eprintln!("Could not remove a Codex package staging directory.");
            }
        }
    }
}
