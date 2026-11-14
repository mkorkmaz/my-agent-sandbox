use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    pub fn generate() -> Result<Self, String> {
        let mut bytes = [0_u8; 16];
        if unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
            return Err("Could not create a new agent session ID.".into());
        }
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let value: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        Self::new(&format!(
            "{}-{}-{}-{}-{}",
            &value[..8],
            &value[8..12],
            &value[12..16],
            &value[16..20],
            &value[20..]
        ))
    }

    pub fn new(value: &str) -> Result<Self, String> {
        if !crate::conversations::valid_id(value) {
            return Err("The agent session ID must be a UUID.".into());
        }
        Ok(Self(value.into()))
    }

    pub(crate) fn argument(&self) -> String {
        self.0.clone()
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(serde::de::Error::custom)
    }
}
