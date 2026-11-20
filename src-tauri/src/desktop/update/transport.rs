use std::io::{self, Read};
use std::process::{Command, Stdio};
use tauri::Url;

pub(super) const MANIFEST_LIMIT: usize = 256 * 1024;
pub(super) const ARCHIVE_LIMIT: usize = 512 * 1024 * 1024;

#[cfg(test)]
pub(super) fn loopback_fixture(address: &Url) -> bool {
    address.scheme() == "http"
        && address.host_str() == Some("127.0.0.1")
        && address.port().is_some()
}

pub(super) fn fetch(
    address: &Url,
    limit: usize,
    timeout_seconds: u32,
    download: bool,
    mut progress: impl FnMut(usize),
) -> Result<Vec<u8>, String> {
    let mut curl = Command::new("/usr/bin/curl");
    curl.arg("--disable");
    let protocol = "=https";
    #[cfg(test)]
    let protocol = if loopback_fixture(address) {
        curl.args(["--noproxy", "127.0.0.1"]);
        "=http"
    } else {
        protocol
    };
    // Use the Mac's trust store. Disable curlrc, URL globbing and non-HTTPS redirects.
    let mut child = curl
        .args([
            "--globoff",
            "--silent",
            "--fail",
            "--location",
            "--max-redirs",
            "5",
            "--proto",
            protocol,
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "5",
            "--max-time",
            &timeout_seconds.to_string(),
            "--speed-limit",
            "1",
            "--speed-time",
            "15",
            "--max-filesize",
            &limit.to_string(),
            "--",
            address.as_str(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not start the network request. {error}"))?;
    let result: Result<Vec<u8>, String> = (|| {
        let mut stream = child.stdout.take().expect("piped curl output");
        let mut bytes = Vec::new();
        let mut buffer = [0; 64 * 1024];
        loop {
            let length = match stream.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result.map_err(|error| error.to_string())?,
            };
            if length == 0 {
                return Ok(bytes);
            }
            if length > limit - bytes.len() {
                return Err("The update response is too large.".into());
            }
            bytes.extend_from_slice(&buffer[..length]);
            progress(bytes.len());
        }
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    let bytes = result?;
    if !status.success() || bytes.is_empty() {
        let action = if download {
            "download the update"
        } else {
            "check for updates"
        };
        return Err(format!(
            "Could not {action}. Check the network connection and try again."
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn a_truncated_archive_is_a_failed_download() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = Url::parse(&format!(
            "http://{}/Tugboat.app.tar.gz",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = String::new();
            let mut reader = BufReader::new(&mut socket);
            reader.read_line(&mut request).unwrap();
            assert_eq!(
                request.split_whitespace().nth(1),
                Some("/Tugboat.app.tar.gz")
            );
            loop {
                request.clear();
                if reader.read_line(&mut request).unwrap() == 0 || request == "\r\n" {
                    break;
                }
            }
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\na"
            )
            .unwrap();
        });
        let mut received = 0;
        assert!(fetch(&address, ARCHIVE_LIMIT, 2, true, |bytes| received = bytes).is_err());
        assert_eq!(received, 1);
        server.join().unwrap();
    }
}
