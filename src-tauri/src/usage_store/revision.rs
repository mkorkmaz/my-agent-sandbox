//! Serialize the existing revision contract without allocating all request rows.
use crate::usage_store::{codecs, error};
use crate::{agents::AgentKind, conversations::SessionRecord, usage::Cursor};
use rusqlite::Connection;
use serde::ser::{Error, SerializeMap, SerializeSeq, SerializeStruct};
use serde::{Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

struct HashWriter(Sha256);
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct CursorStates<'a> {
    connection: &'a Connection,
    record: i64,
}
impl Serialize for CursorStates<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        let mut statement = self.connection.prepare("SELECT source_path,cursor_json FROM source_cursors WHERE record_id=?1 ORDER BY source_path").map_err(S::Error::custom)?;
        let rows = statement
            .query_map([self.record], |row| {
                let bytes: Vec<u8> = row.get(0)?;
                Ok((
                    PathBuf::from(OsString::from_vec(bytes)),
                    codecs::decode::<Cursor>(row, "cursor_json")?,
                ))
            })
            .map_err(S::Error::custom)?;
        for row in rows {
            let (path, cursor) = row.map_err(S::Error::custom)?;
            map.serialize_entry(&path, &cursor.revision_state())?;
        }
        map.end()
    }
}

struct Requests<'a> {
    connection: &'a Connection,
    turn: i64,
}
impl Serialize for Requests<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(None)?;
        let mut statement = self
            .connection
            .prepare("SELECT * FROM usage_requests WHERE turn_key=?1 ORDER BY ordinal")
            .map_err(S::Error::custom)?;
        for request in statement
            .query_map([self.turn], codecs::read_request)
            .map_err(S::Error::custom)?
        {
            sequence.serialize_element(&request.map_err(S::Error::custom)?)?;
        }
        sequence.end()
    }
}

struct Turn<'a> {
    connection: &'a Connection,
    key: i64,
}
impl Serialize for Turn<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let metadata = self.connection.query_row("SELECT native_turn_id,status,origin_thread_id,root_turn_id,parent_turn_id FROM usage_turns WHERE turn_key=?1", [self.key], |row| Ok((row.get::<_,String>(0)?, row.get::<_,String>(1)?, row.get::<_,Option<String>>(2)?, row.get::<_,Option<String>>(3)?, row.get::<_,Option<String>>(4)?))).map_err(S::Error::custom)?;
        let mut value = serializer.serialize_struct("TurnUsage", 6)?;
        value.serialize_field("id", &metadata.0)?;
        value.serialize_field("status", &metadata.1)?;
        value.serialize_field(
            "requests",
            &Requests {
                connection: self.connection,
                turn: self.key,
            },
        )?;
        value.serialize_field("originThreadId", &metadata.2)?;
        value.serialize_field("rootTurnId", &metadata.3)?;
        value.serialize_field("parentTurnId", &metadata.4)?;
        value.end()
    }
}

struct Turns<'a> {
    connection: &'a Connection,
    record: i64,
}
impl Serialize for Turns<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        let mut statement = self.connection.prepare("SELECT turn_key,bucket_key FROM usage_turns WHERE record_id=?1 ORDER BY bucket_key").map_err(S::Error::custom)?;
        let rows = statement
            .query_map([self.record], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(S::Error::custom)?;
        for row in rows {
            let (key, bucket) = row.map_err(S::Error::custom)?;
            map.serialize_entry(
                &bucket,
                &Turn {
                    connection: self.connection,
                    key,
                },
            )?;
        }
        map.end()
    }
}

pub(in crate::usage_store) fn revision(
    connection: &Connection,
    record_id: i64,
    metadata: &SessionRecord,
    available: bool,
) -> Result<String, String> {
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(
        &mut writer,
        &(
            metadata.parser_version,
            CursorStates {
                connection,
                record: record_id,
            },
            &metadata.lineage,
            &metadata.lineage_problem,
            &metadata.warning,
            available,
            &metadata.source_root_id,
            &metadata.title,
            (metadata.agent_kind == Some(AgentKind::ClaudeCode)).then_some(Turns {
                connection,
                record: record_id,
            }),
        ),
    )
    .map_err(error)?;
    Ok(crate::hash::hex(writer.0.finalize()))
}
