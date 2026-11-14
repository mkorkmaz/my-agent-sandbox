pub enum RecordSchema<T> {
    Current(T),
    Upgrade(T),
}

impl<T> RecordSchema<T> {
    pub fn into_record(self) -> T {
        match self {
            Self::Current(record) | Self::Upgrade(record) => record,
        }
    }
}
