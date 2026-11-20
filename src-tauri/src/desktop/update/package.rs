use flate2::read::GzDecoder;
use std::io::{self, Cursor, Read};
use std::path::{Component, Path};

const UNPACKED_LIMIT: u64 = 1024 * 1024 * 1024;
const INFO_LIMIT: u64 = 256 * 1024;
const ARM64_MACH_HEADER: [u8; 8] = [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0x00, 0x00, 0x01];

pub(super) fn validate(bytes: &[u8], version: &str, identifier: &str) -> Result<(), String> {
    inspect(bytes, version, identifier)
        .map_err(|error| format!("The signed update package is invalid. {error}"))
}

fn inspect(
    bytes: &[u8],
    version: &str,
    identifier: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let reader = GzDecoder::new(bytes).take(UNPACKED_LIMIT);
    let mut archive = tar::Archive::new(reader);
    let mut metadata = false;
    let mut executable = false;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if !path.starts_with("Tugboat.app")
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err("The archive contains an unexpected path.".into());
        }
        if path == Path::new("Tugboat.app/Contents/Info.plist") {
            if metadata || !entry.header().entry_type().is_file() || entry.size() > INFO_LIMIT {
                return Err("The app information in the archive is invalid.".into());
            }
            let mut contents = Vec::new();
            entry.read_to_end(&mut contents)?;
            let fields: plist::Dictionary = plist::from_reader(Cursor::new(contents))?;
            let field = |name| fields.get(name).and_then(plist::Value::as_string);
            metadata = field("CFBundleShortVersionString") == Some(version)
                && field("CFBundleIdentifier") == Some(identifier)
                && field("CFBundleExecutable") == Some("tugboat");
            if !metadata {
                return Err("The app in the archive has a different version or identity.".into());
            }
        }
        if path == Path::new("Tugboat.app/Contents/MacOS/tugboat") {
            if executable
                || !entry.header().entry_type().is_file()
                || entry.header().mode()? & 0o111 == 0
            {
                return Err("The Tugboat executable in the archive is invalid.".into());
            }
            let mut header = [0; ARM64_MACH_HEADER.len()];
            entry.read_exact(&mut header)?;
            if header != ARM64_MACH_HEADER {
                return Err("The archive does not contain an Apple silicon executable.".into());
            }
            executable = true;
        }
        io::copy(&mut entry, &mut io::sink())?;
    }
    // Read through the gzip footer to check integrity, even after the end of the tar file.
    let mut remaining = archive.into_inner();
    io::copy(&mut remaining, &mut io::sink())?;
    if remaining.limit() == 0 || !metadata || !executable {
        return Err("The archive is too large or is missing required files.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};

    fn archive(version: &str, identifier: &str, add_executable: bool) -> Vec<u8> {
        let mut fields = plist::Dictionary::new();
        fields.insert("CFBundleIdentifier".into(), identifier.into());
        fields.insert("CFBundleShortVersionString".into(), version.into());
        fields.insert("CFBundleExecutable".into(), "tugboat".into());
        let mut contents = Vec::new();
        plist::to_writer_xml(&mut contents, &fields).unwrap();
        let gzip = GzEncoder::new(Vec::new(), Compression::default());
        let mut archive = tar::Builder::new(gzip);
        let mut append = |path: &str, bytes: &[u8]| {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            archive.append_data(&mut header, path, bytes).unwrap();
        };
        append("Tugboat.app/Contents/Info.plist", &contents);
        if add_executable {
            append("Tugboat.app/Contents/MacOS/tugboat", &ARM64_MACH_HEADER);
        }
        archive.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn incomplete_and_mislabeled_archives_fail_before_the_shutdown_gate() {
        const VERSION: &str = "0.5.1";
        const IDENTIFIER: &str = "sh.brig.desktop";
        let valid = archive(VERSION, IDENTIFIER, true);
        assert!(validate(&valid, VERSION, IDENTIFIER).is_ok());
        for package in [
            archive(VERSION, IDENTIFIER, false),
            archive("0.5.0", IDENTIFIER, true),
            archive(VERSION, "sh.brig.desktop.dev", true),
            valid[..valid.len() - 4].to_vec(),
        ] {
            assert!(validate(&package, VERSION, IDENTIFIER).is_err());
        }
    }
}
