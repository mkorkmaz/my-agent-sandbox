use crate::platform::{command, host, runtime};
use std::{collections::BTreeMap, path::PathBuf, sync::atomic::AtomicBool, time::Duration};

const UNAVAILABLE: &str =
    "Brig could not access the Mac Keychain. Unlock the Keychain or allow its prompt, then retry.";

pub(super) struct Keychain {
    pub(super) executable: PathBuf,
    pub(super) environment: BTreeMap<String, String>,
}

impl Keychain {
    pub(super) fn host() -> Result<Self, String> {
        runtime::validate()?;
        let executable =
            runtime::tool("brig").ok_or("The bundled Brig executable is unavailable.")?;
        let mut environment = command::environment();
        if !environment
            .get("HOME")
            .is_some_and(|home| std::path::Path::new(home).is_absolute())
        {
            return Err(UNAVAILABLE.into());
        }
        environment.insert("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into());
        Ok(Self {
            executable,
            environment,
        })
    }

    pub(super) fn contains(&self, name: &str) -> Result<bool, String> {
        let output = self.run(&["--json", "secret", "ls"], None)?;
        let doc = host::envelope(&output, "SecretList").map_err(|_| UNAVAILABLE)?;
        let rows = doc["data"].as_array().ok_or(UNAVAILABLE)?;
        if rows.iter().any(|row| !row["name"].is_string()) {
            return Err(UNAVAILABLE.into());
        }
        Ok(rows.iter().any(|row| row["name"] == name))
    }

    pub(super) fn write(&self, name: &str, token: Vec<u8>, exists: bool) -> Result<(), String> {
        let verb = if exists { "update" } else { "create" };
        let output = self.run(&["secret", verb, name], Some(token))?;
        if output.exit_code != Some(0) || !self.contains(name)? {
            return Err(UNAVAILABLE.into());
        }
        Ok(())
    }

    pub(super) fn delete(&self, name: &str) -> Result<(), String> {
        let output = self.run(&["secret", "delete", name, "-y"], None)?;
        if output.exit_code != Some(0)
            && !output.stderr.contains(&format!("no secret named {name:?}"))
        {
            return Err(UNAVAILABLE.into());
        }
        if self.contains(name)? {
            return Err(UNAVAILABLE.into());
        }
        Ok(())
    }

    fn run(&self, args: &[&str], input: Option<Vec<u8>>) -> Result<command::CommandOutput, String> {
        let args: Vec<_> = args.iter().map(|arg| (*arg).into()).collect();
        let output = command::run_with_input(
            &self.executable,
            &args,
            None,
            &self.environment,
            Duration::from_secs(30),
            &AtomicBool::new(false),
            input,
        )
        .map_err(|_| UNAVAILABLE)?;
        if output.timed_out || output.cancelled || output.output_exceeded {
            return Err(UNAVAILABLE.into());
        }
        Ok(output)
    }
}

pub(super) fn token(value: String) -> Result<Vec<u8>, String> {
    if !(20..=512).contains(&value.len())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    {
        return Err("Enter the GitHub access token without spaces or line breaks.".into());
    }
    let mut bytes = value.into_bytes();
    bytes.push(b'\n');
    Ok(bytes)
}
