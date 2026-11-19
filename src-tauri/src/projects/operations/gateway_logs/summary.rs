use regex::Regex;
use std::{net::Ipv4Addr, sync::LazyLock};

pub(super) const LINE_LIMIT: usize = 100;
pub(super) const HIDDEN_ENTRY: &str = "[Gateway log details removed]";
const DNS_QUERY: &str = "egress: refused a DNS query";
const DNS_HOST: &str = "egress: could not resolve an allowed host";
const STARTED: &str = "network-gateway: started";

static HULL_RECORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^time="([0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:Z|[+-][0-9]{2}:[0-9]{2}))" level=(info|warning|error|debug) msg="(.*)"(?: subsystem=hull)?$"#).unwrap()
});
static CONTROLS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\x1b\[|\u{009b})[0-?]*[ -/]*[@-~]|\x1b\][^\n\x07\x1b]*(?:\x07|\x1b\\)|[\p{Cc}\p{Cf}&&[^\n\t]]").unwrap()
});
static FORWARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^network-gateway: (?:published|withdrew) (?:udp:)?127\.0\.0\.1:([0-9]{1,5})=([0-9.]+):([0-9]{1,5})$").unwrap()
});
static DENIAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^egress: rejected (?:tcp|udp) from ([0-9.]+) to ([0-9.]+):([0-9]{1,5})$").unwrap()
});

fn valid_port(port: &str) -> bool {
    port.parse::<u16>().is_ok_and(|port| port > 0)
}

fn valid_forward(message: &str) -> bool {
    FORWARD.captures(message).is_some_and(|forward| {
        valid_port(&forward[1]) && forward[2].parse::<Ipv4Addr>().is_ok() && valid_port(&forward[3])
    })
}

fn valid_denial(message: &str) -> bool {
    DENIAL.captures(message).is_some_and(|denial| {
        denial[1].parse::<Ipv4Addr>().is_ok()
            && denial[2].parse::<Ipv4Addr>().is_ok()
            && valid_port(&denial[3])
    })
}

fn safe_message(message: &str) -> Option<&str> {
    if valid_forward(message)
        || valid_denial(message)
        || [DNS_QUERY, DNS_HOST, STARTED].contains(&message)
    {
        return Some(message);
    }
    if message.starts_with("egress: refused the ") && message.contains(" query ") {
        return Some(DNS_QUERY);
    }
    if message.starts_with("egress: cannot resolve the allowed host ") {
        return Some(DNS_HOST);
    }
    if message.starts_with("network-gateway on ") {
        return Some(STARTED);
    }
    None
}

fn summarize(line: &str) -> Option<String> {
    let (_, [timestamp, level, message]) = HULL_RECORD.captures(line)?.extract();
    // The tail can start inside a multiline credential. Only approved fields reach the view.
    Some(format!("{timestamp} {level} {}", safe_message(message)?))
}

pub(super) fn redact(stdout: &str) -> Option<String> {
    let text = stdout.replace("\r\n", "\n");
    let text = CONTROLS.replace_all(&text, "");
    if text.trim().is_empty() {
        return None;
    }
    let mut entries: Vec<_> = text
        .lines()
        .rev()
        .take(LINE_LIMIT)
        .map(|line| summarize(line).unwrap_or_else(|| HIDDEN_ENTRY.into()))
        .collect();
    entries.reverse();
    Some(entries.join("\n"))
}

fn is_summary(line: &str) -> bool {
    if line == HIDDEN_ENTRY {
        return true;
    }
    let mut fields = line.splitn(3, ' ');
    let (Some(timestamp), Some(level), Some(message)) =
        (fields.next(), fields.next(), fields.next())
    else {
        return false;
    };
    let record = format!("time=\"{timestamp}\" level={level} msg=\"{message}\"");
    summarize(&record).as_deref() == Some(line)
}

pub(super) fn can_copy(text: &str) -> bool {
    text.len() <= LINE_LIMIT * 160
        && text.split('\n').count() <= LINE_LIMIT
        && text.split('\n').all(is_summary)
}
