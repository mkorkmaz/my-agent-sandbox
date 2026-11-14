//! Reject excessive nesting and decoded strings before serde allocates them.
use super::leaf::LEAF_LIMIT;
use std::io::{self, Read};

#[derive(Clone, Copy)]
enum Surrogate { None, High }

#[derive(Clone, Copy, Default)]
enum StringPhase {
    #[default]
    Outside,
    Text(Surrogate),
    Escape(Surrogate),
    Unicode { remaining: u8, unit: u16, previous: Surrogate },
}

#[derive(Default)]
struct StringState {
    phase: StringPhase,
    decoded: usize,
}

impl StringState {
    fn add(&mut self, count: usize) -> io::Result<()> {
        self.decoded = self.decoded.checked_add(count)
            .ok_or_else(|| invalid("Decoded legacy string length overflow."))?;
        if self.decoded > LEAF_LIMIT {
            return Err(invalid("Decoded legacy string exceeds 64 MiB."));
        }
        Ok(())
    }
    fn unicode(&mut self, byte: u8, remaining: u8, unit: u16, previous: Surrogate) -> io::Result<()> {
        let digit = (byte as char).to_digit(16)
            .ok_or_else(|| invalid("Invalid JSON Unicode escape."))?;
        let unit = (unit << 4) | digit as u16;
        if remaining > 1 {
            self.phase = StringPhase::Unicode { remaining: remaining - 1, unit, previous };
            return Ok(());
        }
        let count = match unit {
            0xdc00..=0xdfff if matches!(previous, Surrogate::High) => 1,
            0..=0x7f => 1,
            0x80..=0x7ff => 2,
            _ => 3,
        };
        let surrogate = if (0xd800..=0xdbff).contains(&unit) { Surrogate::High } else { Surrogate::None };
        self.phase = StringPhase::Text(surrogate);
        self.add(count)
    }
    fn consume(&mut self, byte: u8) -> io::Result<()> {
        match self.phase {
            StringPhase::Unicode { remaining, unit, previous } => self.unicode(byte, remaining, unit, previous),
            StringPhase::Escape(previous) if byte == b'u' => {
                self.phase = StringPhase::Unicode { remaining: 4, unit: 0, previous };
                Ok(())
            }
            StringPhase::Escape(_) => {
                self.phase = StringPhase::Text(Surrogate::None);
                self.add(1)
            }
            StringPhase::Text(previous) if byte == b'\\' => {
                self.phase = StringPhase::Escape(previous);
                Ok(())
            }
            StringPhase::Text(_) if byte == b'"' => {
                self.phase = StringPhase::Outside;
                Ok(())
            }
            StringPhase::Text(_) => {
                self.phase = StringPhase::Text(Surrogate::None);
                self.add(1)
            }
            StringPhase::Outside => Err(invalid("No JSON string is open.")),
        }
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(in crate::usage_store) struct GuardedRead<R> {
    input: R,
    string: StringState,
    depth: usize,
}
impl<R: Read> GuardedRead<R> {
    pub fn new(input: R) -> Self {
        Self {
            input,
            string: StringState::default(),
            depth: 0,
        }
    }
    fn inspect(&mut self, byte: u8) -> io::Result<()> {
        if !matches!(self.string.phase, StringPhase::Outside) {
            return self.string.consume(byte);
        }
        if byte == b'"' {
            self.string = StringState {
                phase: StringPhase::Text(Surrogate::None),
                ..StringState::default()
            };
            return Ok(());
        }
        if matches!(byte, b'[' | b'{') {
            self.depth += 1;
            if self.depth > 128 {
                return Err(invalid("Legacy JSON exceeds 128 nesting levels."));
            }
        }
        if matches!(byte, b']' | b'}') {
            self.depth = self
                .depth
                .checked_sub(1)
                .ok_or_else(|| invalid("Unbalanced legacy JSON."))?;
        }
        Ok(())
    }
}
impl<R: Read> Read for GuardedRead<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.input.read(buffer)?;
        for &byte in &buffer[..count] {
            self.inspect(byte)?;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_guard_bounds_nesting_and_counts_escaped_unicode_bytes() {
        for (depth, allowed) in [(128, true), (129, false)] {
            let json = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
            assert_eq!(
                GuardedRead::new(json.as_bytes())
                    .read_to_end(&mut Vec::new())
                    .is_ok(),
                allowed
            );
        }
        let mut guard = GuardedRead::new(br#""\u0041\u00e9\ud83d\ude00""#.as_slice());
        guard.read_to_end(&mut Vec::new()).unwrap();
        assert_eq!(guard.string.decoded, 7);
        let mut string = StringState {
            phase: StringPhase::Text(Surrogate::None),
            decoded: LEAF_LIMIT - 1,
            ..StringState::default()
        };
        string.consume(b'x').unwrap();
        assert!(string.consume(b'x').is_err());
    }
}
