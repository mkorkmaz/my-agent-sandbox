#[derive(Debug, PartialEq)]
pub struct ChatTitle {
    pub prefix: Option<String>,
    pub busy: bool,
}

impl ChatTitle {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        let (selector, title) = text.split_once(';')?;
        if !["0", "2"].contains(&selector) {
            return None;
        }
        let mut parts = title.split('|').map(str::trim);
        let identity = parts.next()?.trim_end_matches(['…', '.']);
        // Codex versions can shorten the UUID in their terminal title.
        // Resolve a shortened identity only against a unique saved conversation.
        let prefix = (identity.len() >= 24
            && identity.len() <= 36
            && identity.bytes().enumerate().all(|(index, byte)| {
                if [8, 13, 18, 23].contains(&index) {
                    return byte == b'-';
                }
                byte.is_ascii_hexdigit()
            }))
        .then(|| identity.to_ascii_lowercase());
        let busy = parts.any(|part| {
            matches!(
                part,
                "Starting" | "Working" | "Thinking" | "Running" | "Compacting"
            )
        });
        Some(Self { prefix, busy })
    }
}

#[derive(Default)]
pub struct TitleParser {
    state: u8,
    bytes: Vec<u8>,
}

impl TitleParser {
    pub fn feed(&mut self, output: &[u8]) -> Vec<ChatTitle> {
        let mut titles = Vec::new();
        for &byte in output {
            match (self.state, byte) {
                (0, 0x1b) => self.state = 1,
                (1, b']') => {
                    self.bytes.clear();
                    self.state = 2;
                }
                (2, 0x07) | (3, b'\\') => {
                    if let Some(title) = ChatTitle::parse(&self.bytes) {
                        titles.push(title);
                    }
                    self.state = 0;
                }
                (2, 0x1b) => self.state = 3,
                (2, _) if self.bytes.len() < 4096 => self.bytes.push(byte),
                (_, 0x1b) => self.state = 1,
                _ => self.state = 0,
            }
        }
        titles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7";

    #[test]
    fn reads_split_titles_with_both_terminators_and_ignores_terminal_text() {
        let output =
            format!("plain {ID} | Working\x1b]0;{ID} | Thinking\x07\x1b]2;{ID} | Ready\x1b\\");
        let mut parser = TitleParser::default();
        let titles: Vec<_> = output
            .as_bytes()
            .chunks(3)
            .flat_map(|chunk| parser.feed(chunk))
            .collect();
        assert_eq!(
            titles,
            vec![
                ChatTitle {
                    prefix: Some(ID.into()),
                    busy: true
                },
                ChatTitle {
                    prefix: Some(ID.into()),
                    busy: false
                }
            ]
        );
    }

    #[test]
    fn accepts_shortened_ids_without_reading_other_osc_payloads() {
        let mut parser = TitleParser::default();
        let title = format!("\x1b]52;c;private\x07\x1b]0;{}… | Working\x07", &ID[..31]);
        assert_eq!(
            parser.feed(title.as_bytes()),
            vec![ChatTitle {
                prefix: Some(ID[..31].into()),
                busy: true
            }]
        );
        let oversized = format!("\x1b]0;{}\x07\x1b]0;{ID} | Ready\x07", "a".repeat(5000));
        assert_eq!(parser.feed(oversized.as_bytes()).len(), 1);
    }

    #[test]
    fn reads_codex_0167_ascii_truncation_and_starting_state() {
        let mut parser = TitleParser::default();
        let output = format!("\x1b]0;{}... | Starting\x07", &ID[..25]);
        assert_eq!(
            parser.feed(output.as_bytes()),
            vec![ChatTitle {
                prefix: Some(ID[..25].into()),
                busy: true
            }]
        );
        let output = format!("\x1b]0;{}... | Ready\x07", &ID[..25]);
        assert!(!parser.feed(output.as_bytes())[0].busy);
    }
}
