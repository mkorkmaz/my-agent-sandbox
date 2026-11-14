use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct PolicyEditor {
    directory: PathBuf,
    pub executable: PathBuf,
    pub source: PathBuf,
}

impl PolicyEditor {
    pub fn create(name: &str, hosts: &[String]) -> Result<Self, String> {
        if !crate::desktop::settings::valid_identifier(name)
            || hosts.len() > 64
            || hosts
                .iter()
                .any(|host| !crate::desktop::settings::valid_host(host))
        {
            return Err("Use a valid policy name and allowed hosts.".into());
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        // Brig v0.5.2 splits EDITOR with strings.Fields. Use a private, space-free path.
        let directory = PathBuf::from(format!(
            "/private/tmp/brig-desktop-policy-{}-{nonce}",
            std::process::id()
        ));
        DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|error| error.to_string())?;
        let editor = Self {
            executable: directory.join("editor"),
            source: directory.join("policy.yaml"),
            directory,
        };
        let mut source = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&editor.source)
            .map_err(|error| error.to_string())?;
        let mut document = format!("apiVersion: brig.sh/v1alpha1\nkind: Policy\nmetadata:\n  name: {name}\n  description: Prepared by Tugboat\nspec:\n  egress:\n    default: deny\n    allow:");
        if hosts.is_empty() {
            document.push_str(" []\n");
        }
        for host in hosts {
            document.push_str(&format!("\n      - host: {host}"));
        }
        document.push('\n');
        source
            .write_all(document.as_bytes())
            .map_err(|error| error.to_string())?;
        let mut executable = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&editor.executable)
            .map_err(|error| error.to_string())?;
        executable
            .write_all(b"#!/bin/sh\nexec /bin/cp \"$BRIG_DESKTOP_POLICY_SOURCE\" \"$1\"\n")
            .map_err(|error| error.to_string())?;
        Ok(editor)
    }
}

impl Drop for PolicyEditor {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.executable);
        let _ = fs::remove_file(&self.source);
        let _ = fs::remove_dir(&self.directory);
    }
}
