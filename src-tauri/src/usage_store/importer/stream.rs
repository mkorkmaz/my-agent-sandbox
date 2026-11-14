use super::{inventory, leaf};
use crate::conversations::SessionRecord;
use crate::usage::{Cursor, RequestUsage, TurnUsage};
use crate::usage_store::{codecs, error, import_rollup};
use rusqlite::{params, Connection};
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::cell::Cell;
use std::fmt;
use std::io::BufReader;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Counts {
    pub records: u64,
    pub turns: u64,
    pub requests: u64,
}

struct Context<'a> {
    connection: &'a Connection,
    environment_key: i64,
    compare: bool,
    counts: Cell<Counts>,
}

impl Context<'_> {
    fn unique<E: serde::de::Error>(&self, scope: &str, key: &str) -> Result<(), E> {
        self.connection
            .execute(
                "INSERT INTO temp.import_map_keys(scope,map_key) VALUES(?1,?2)",
                params![scope, key],
            )
            .map_err(|_| E::custom("Duplicate legacy map key."))?;
        Ok(())
    }
    fn group<E: serde::de::Error>(&self, id: &str, bucket: bool) -> Result<i64, E> {
        if !self.compare {
            return codecs::ensure_group(self.connection, self.environment_key, id, bucket)
                .map_err(E::custom);
        }
        let (key, has_bucket): (i64, bool) = self.connection.query_row(
            "SELECT session_key,has_variants_bucket FROM session_groups WHERE environment_key=?1 AND native_session_id=?2",
            params![self.environment_key,id], |row| Ok((row.get(0)?,row.get(1)?)),
        ).map_err(E::custom)?;
        if bucket && !has_bucket {
            return Err(E::custom("Legacy variants bucket did not round-trip."));
        }
        Ok(key)
    }
    fn record<E: serde::de::Error>(&self, group: i64, variant: Option<&str>) -> Result<i64, E> {
        if !self.compare {
            return codecs::insert_record(
                self.connection,
                group,
                variant,
                &SessionRecord::default(),
            )
            .map_err(E::custom);
        }
        self.connection
            .query_row(
                "SELECT record_id FROM session_records WHERE session_key=?1 AND variant_key IS ?2",
                params![group, variant],
                |row| row.get(0),
            )
            .map_err(E::custom)
    }
}

fn same<T: serde::Serialize, E: serde::de::Error>(input: &T, stored: &T) -> Result<(), E> {
    if serde_json::to_value(input).map_err(E::custom)?
        != serde_json::to_value(stored).map_err(E::custom)?
    {
        return Err(E::custom(
            "Typed legacy value did not round-trip through SQLite.",
        ));
    }
    Ok(())
}

struct Ledger<'a>(&'a Context<'a>);
impl<'de> DeserializeSeed<'de> for Ledger<'_> {
    type Value = (u32, Vec<String>);
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for Ledger<'_> {
    type Value = (u32, Vec<String>);
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a legacy usage ledger")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut version = None;
        let mut sessions = false;
        let mut warnings = Vec::new();
        while let Some(field) = map.next_key::<String>()? {
            self.0.unique::<A::Error>("ledger", &field)?;
            match field.as_str() {
                "schemaVersion" => version = Some(map.next_value::<u32>()?),
                "sessions" => {
                    map.next_value_seed(Sessions(self.0))?;
                    sessions = true;
                }
                "variants" => {
                    map.next_value_seed(Variants(self.0))?;
                }
                "warnings" => {
                    warnings =
                        leaf::typed(leaf::next_leaf(&mut map)?).map_err(serde::de::Error::custom)?
                }
                _ => return Err(serde::de::Error::custom("Unsupported legacy ledger field.")),
            }
        }
        if !sessions || !matches!(version, Some(1 | 2)) {
            return Err(serde::de::Error::custom(
                "Missing sessions or unsupported legacy version.",
            ));
        }
        Ok((version.expect("checked version"), warnings))
    }
}

struct Sessions<'a>(&'a Context<'a>);
impl<'de> DeserializeSeed<'de> for Sessions<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for Sessions<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a session map")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(id) = map.next_key::<String>()? {
            self.0.unique::<A::Error>("sessions", &id)?;
            let group = self.0.group::<A::Error>(&id, false)?;
            let record = self.0.record::<A::Error>(group, None)?;
            map.next_value_seed(Record {
                context: self.0,
                id: &id,
                record,
            })?;
        }
        Ok(())
    }
}

struct Variants<'a>(&'a Context<'a>);
impl<'de> DeserializeSeed<'de> for Variants<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for Variants<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a variants map")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(id) = map.next_key::<String>()? {
            self.0.unique::<A::Error>("variants", &id)?;
            let group = self.0.group::<A::Error>(&id, true)?;
            map.next_value_seed(VariantRecords {
                context: self.0,
                id: &id,
                group,
            })?;
        }
        Ok(())
    }
}

struct VariantRecords<'a> {
    context: &'a Context<'a>,
    id: &'a str,
    group: i64,
}
impl<'de> DeserializeSeed<'de> for VariantRecords<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for VariantRecords<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("historical guest slots")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let scope = format!("variants:{}", self.group);
        while let Some(variant) = map.next_key::<String>()? {
            self.context.unique::<A::Error>(&scope, &variant)?;
            let record = self
                .context
                .record::<A::Error>(self.group, Some(&variant))?;
            map.next_value_seed(Record {
                context: self.context,
                id: self.id,
                record,
            })?;
        }
        Ok(())
    }
}

struct Record<'a> {
    context: &'a Context<'a>,
    id: &'a str,
    record: i64,
}
impl<'de> DeserializeSeed<'de> for Record<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for Record<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a session record")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut metadata = Map::new();
        let scope = format!("record:{}", self.record);
        while let Some(field) = map.next_key::<String>()? {
            self.context.unique::<A::Error>(&scope, &field)?;
            match field.as_str() {
                "turns" => {
                    map.next_value_seed(Turns {
                        context: self.context,
                        record: self.record,
                    })?;
                    metadata.insert(field, Value::Object(Map::new()));
                }
                "cursors" => {
                    map.next_value_seed(Cursors {
                        context: self.context,
                        record: self.record,
                    })?;
                    metadata.insert(field, Value::Object(Map::new()));
                }
                _ if matches!(
                    field.as_str(),
                    "agentKind"
                        | "claudeSource"
                        | "guestNamespace"
                        | "title"
                        | "updatedAt"
                        | "state"
                        | "warning"
                        | "parserVersion"
                        | "available"
                        | "lineage"
                        | "lineageProblem"
                        | "sourceRootId"
                        | "projectExclusion"
                ) =>
                {
                    metadata.insert(field, leaf::next_leaf(&mut map)?);
                }
                _ => {
                    return Err(serde::de::Error::custom(
                        "Unsupported legacy session field.",
                    ))
                }
            }
        }
        let record: SessionRecord =
            leaf::typed(Value::Object(metadata)).map_err(serde::de::Error::custom)?;
        if self.context.compare {
            for (table, scope) in [
                ("usage_turns", format!("turns:{}", self.record)),
                ("source_cursors", format!("cursors:{}", self.record)),
            ] {
                let actual: i64 = self
                    .context
                    .connection
                    .query_row(
                        &format!("SELECT count(*) FROM {table} WHERE record_id=?1"),
                        [self.record],
                        |row| row.get(0),
                    )
                    .map_err(serde::de::Error::custom)?;
                let expected: i64 = self
                    .context
                    .connection
                    .query_row(
                        "SELECT count(*) FROM temp.import_map_keys WHERE scope=?1",
                        [scope],
                        |row| row.get(0),
                    )
                    .map_err(serde::de::Error::custom)?;
                if actual != expected {
                    return Err(serde::de::Error::custom(
                        "Stored turn or cursor count differs from the source.",
                    ));
                }
            }
        }
        if self.context.compare {
            let stored = self
                .context
                .connection
                .query_row(
                    "SELECT * FROM session_records WHERE record_id=?1",
                    [self.record],
                    codecs::read_record_metadata,
                )
                .map_err(serde::de::Error::custom)?;
            same::<_, A::Error>(&record, &stored)?;
        }
        if !self.context.compare {
            codecs::update_record_metadata(self.context.connection, self.record, &record)
                .map_err(serde::de::Error::custom)?;
            let rollup = import_rollup::build(self.context.connection, self.id, self.record)
                .map_err(serde::de::Error::custom)?;
            codecs::write_rollup(self.context.connection, self.record, &rollup)
                .map_err(serde::de::Error::custom)?;
        }
        let mut counts = self.context.counts.get();
        counts.records += 1;
        self.context.counts.set(counts);
        Ok(())
    }
}

struct Turns<'a> {
    context: &'a Context<'a>,
    record: i64,
}
impl<'de> DeserializeSeed<'de> for Turns<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for Turns<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a stored turn map")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let scope = format!("turns:{}", self.record);
        while let Some(bucket) = map.next_key::<String>()? {
            self.context.unique::<A::Error>(&scope, &bucket)?;
            let turn = if self.context.compare {
                self.context
                    .connection
                    .query_row(
                        "SELECT turn_key FROM usage_turns WHERE record_id=?1 AND bucket_key=?2",
                        params![self.record, bucket],
                        |row| row.get(0),
                    )
                    .map_err(serde::de::Error::custom)?
            } else {
                let shell = TurnUsage {
                    id: String::new(),
                    status: Default::default(),
                    requests: Vec::new(),
                    origin_thread_id: None,
                    root_turn_id: None,
                    parent_turn_id: None,
                };
                codecs::insert_turn(self.context.connection, self.record, &bucket, &shell)
                    .map_err(serde::de::Error::custom)?
            };
            map.next_value_seed(Turn {
                context: self.context,
                turn,
            })?;
        }
        Ok(())
    }
}

struct Turn<'a> {
    context: &'a Context<'a>,
    turn: i64,
}
impl<'de> DeserializeSeed<'de> for Turn<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for Turn<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a stored turn")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut metadata = Map::new();
        let scope = format!("turn:{}", self.turn);
        while let Some(field) = map.next_key::<String>()? {
            self.context.unique::<A::Error>(&scope, &field)?;
            if field == "requests" {
                map.next_value_seed(Requests {
                    context: self.context,
                    turn: self.turn,
                })?;
                metadata.insert(field, Value::Array(Vec::new()));
                continue;
            }
            if !matches!(
                field.as_str(),
                "id" | "status" | "originThreadId" | "rootTurnId" | "parentTurnId"
            ) {
                return Err(serde::de::Error::custom("Unsupported legacy turn field."));
            }
            metadata.insert(field, leaf::next_leaf(&mut map)?);
        }
        let turn: TurnUsage =
            leaf::typed(Value::Object(metadata)).map_err(serde::de::Error::custom)?;
        if self.context.compare {
            let stored = self
                .context
                .connection
                .query_row(
                    "SELECT * FROM usage_turns WHERE turn_key=?1",
                    [self.turn],
                    |row| {
                        Ok(TurnUsage {
                            id: row.get("native_turn_id")?,
                            status: row.get("status")?,
                            requests: Vec::new(),
                            origin_thread_id: row.get("origin_thread_id")?,
                            root_turn_id: row.get("root_turn_id")?,
                            parent_turn_id: row.get("parent_turn_id")?,
                        })
                    },
                )
                .map_err(serde::de::Error::custom)?;
            same::<_, A::Error>(&turn, &stored)?;
        }
        if !self.context.compare {
            self.context.connection.execute("UPDATE usage_turns SET native_turn_id=?1,status=?2,origin_thread_id=?3,root_turn_id=?4,parent_turn_id=?5 WHERE turn_key=?6", params![turn.id,turn.status,turn.origin_thread_id,turn.root_turn_id,turn.parent_turn_id,self.turn]).map_err(serde::de::Error::custom)?;
        }
        let mut counts = self.context.counts.get();
        counts.turns += 1;
        self.context.counts.set(counts);
        Ok(())
    }
}

struct Requests<'a> {
    context: &'a Context<'a>,
    turn: i64,
}
impl<'de> DeserializeSeed<'de> for Requests<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for Requests<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("ordered request occurrences")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<(), A::Error> {
        let mut ordinal = 0usize;
        while let Some(value) =
            sequence.next_element_seed(leaf::Leaf::new(&Cell::new(leaf::LEAF_LIMIT)))?
        {
            let request: RequestUsage = leaf::typed(value).map_err(serde::de::Error::custom)?;
            if self.context.compare {
                let stored = self
                    .context
                    .connection
                    .query_row(
                        "SELECT * FROM usage_requests WHERE turn_key=?1 AND ordinal=?2",
                        params![self.turn, ordinal as i64],
                        codecs::read_request,
                    )
                    .map_err(serde::de::Error::custom)?;
                same::<_, A::Error>(&request, &stored)?;
            }
            if !self.context.compare {
                codecs::insert_request(self.context.connection, self.turn, ordinal, &request)
                    .map_err(serde::de::Error::custom)?;
            }
            ordinal = ordinal
                .checked_add(1)
                .ok_or_else(|| serde::de::Error::custom("Request ordinal overflow."))?;
        }
        if self.context.compare {
            let count: i64 = self
                .context
                .connection
                .query_row(
                    "SELECT count(*) FROM usage_requests WHERE turn_key=?1",
                    [self.turn],
                    |row| row.get(0),
                )
                .map_err(serde::de::Error::custom)?;
            if count != ordinal as i64 {
                return Err(serde::de::Error::custom(
                    "Request count differs after import.",
                ));
            }
        }
        let mut counts = self.context.counts.get();
        counts.requests += ordinal as u64;
        self.context.counts.set(counts);
        Ok(())
    }
}

struct Cursors<'a> {
    context: &'a Context<'a>,
    record: i64,
}
impl<'de> DeserializeSeed<'de> for Cursors<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for Cursors<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("typed source cursors")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let scope = format!("cursors:{}", self.record);
        while let Some(path) = map.next_key::<PathBuf>()? {
            self.context.unique::<A::Error>(
                &scope,
                path.to_str()
                    .ok_or_else(|| serde::de::Error::custom("Invalid cursor path."))?,
            )?;
            let cursor: Cursor =
                leaf::typed(leaf::next_leaf(&mut map)?).map_err(serde::de::Error::custom)?;
            if self.context.compare {
                let stored = self.context.connection.query_row("SELECT cursor_json FROM source_cursors WHERE record_id=?1 AND source_path=?2", params![self.record,path.as_os_str().as_bytes()], |row| codecs::decode::<Cursor>(row,"cursor_json")).map_err(serde::de::Error::custom)?;
                same::<_, A::Error>(&cursor, &stored)?;
            }
            if !self.context.compare {
                codecs::insert_cursor(self.context.connection, self.record, &path, &cursor)
                    .map_err(serde::de::Error::custom)?;
            }
        }
        Ok(())
    }
}

pub(super) fn stream(
    connection: &Connection,
    environment_key: i64,
    path: &Path,
    compare: bool,
) -> Result<(u32, Vec<String>, Counts), String> {
    connection.execute_batch("CREATE TEMP TABLE IF NOT EXISTS import_map_keys(scope TEXT NOT NULL,map_key TEXT NOT NULL,PRIMARY KEY(scope,map_key)) WITHOUT ROWID; DELETE FROM temp.import_map_keys;").map_err(error)?;
    let context = Context {
        connection,
        environment_key,
        compare,
        counts: Cell::new(Counts::default()),
    };
    let mut input = serde_json::Deserializer::from_reader(BufReader::with_capacity(
        65_536,
        super::guard::GuardedRead::new(inventory::open_regular(path)?),
    ));
    let (version, warnings) = Ledger(&context).deserialize(&mut input).map_err(error)?;
    input.end().map_err(error)?;
    let counts = context.counts.get();
    let actual = connection.query_row("SELECT (SELECT count(*) FROM session_records r JOIN session_groups g USING(session_key) WHERE g.environment_key=?1),(SELECT count(*) FROM usage_turns t JOIN session_records r USING(record_id) JOIN session_groups g USING(session_key) WHERE g.environment_key=?1),(SELECT count(*) FROM usage_requests q JOIN usage_turns t USING(turn_key) JOIN session_records r USING(record_id) JOIN session_groups g USING(session_key) WHERE g.environment_key=?1)", [environment_key], |row| Ok(Counts { records: count_row(row, 0)?, turns: count_row(row, 1)?, requests: count_row(row, 2)? })).map_err(error)?;
    if counts != actual {
        return Err("Legacy import row counts do not reconcile.".into());
    }
    let groups: i64 = connection
        .query_row(
            "SELECT count(*) FROM session_groups WHERE environment_key=?1",
            [environment_key],
            |row| row.get(0),
        )
        .map_err(error)?;
    let source_groups:i64=connection.query_row("SELECT count(DISTINCT map_key) FROM temp.import_map_keys WHERE scope IN ('sessions','variants')",[],|row|row.get(0)).map_err(error)?;
    let wrong_buckets:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM session_groups g WHERE g.environment_key=?1 AND g.has_variants_bucket != EXISTS(SELECT 1 FROM temp.import_map_keys k WHERE k.scope='variants' AND k.map_key=g.native_session_id))",[environment_key],|row|row.get(0)).map_err(error)?;
    if groups != source_groups || wrong_buckets {
        return Err("Legacy session groups or variants buckets do not reconcile.".into());
    }
    Ok((version, warnings, counts))
}

pub(super) fn count_row(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}
