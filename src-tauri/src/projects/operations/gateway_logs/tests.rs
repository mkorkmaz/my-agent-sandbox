use super::*;
use serde_json::json;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

mod live;

const TIME: &str = "2026-11-19T21:54:17+03:00";

fn record(message: &str) -> String {
    format!("time=\"{TIME}\" level=info msg=\"{message}\" subsystem=hull\n")
}

fn published(port: u16) -> String {
    format!("network-gateway: published 127.0.0.1:{port}=192.168.64.2:23331")
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "tugboat-gateway-logs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let binary = root.join("brig");
        fs::write(
            &binary,
            r#"#!/bin/sh
root=${0%/*}
printf '%s\n' "$*" >> "$root/calls"
test -z "$BRIG_NETWORK" || exit 7
case "$1:$2" in
--json:info)
    test "$#" = 3 && exec /bin/cat "$root/$3-info";;
logs:--gateway)
    test "$#" = 5 && test "$4" = --tail && test "$5" = 100 || exit 9
    test ! -f "$root/$3-overflow" || exec /usr/bin/yes 'token=oversize-fixture'
    test -f "$root/$3-log" || { printf 'token=private-fixture\n' >&2; exit 3; }
    exec /bin/cat "$root/$3-log";;
esac
exit 9
"#,
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        Self(root)
    }

    fn project(&self, registry: &mut Registry, name: &str) -> Project {
        let folder = self.0.join(name);
        fs::create_dir(&folder).unwrap();
        let project = registry.register(&folder).unwrap();
        self.info(
            &project,
            &project,
            "isolated (a network of this sandbox's own)",
        );
        project
    }

    fn write(&self, project: &Project, kind: &str, value: &str) {
        fs::write(
            self.0.join(format!("{}-{kind}", project.reference())),
            value,
        )
        .unwrap();
    }

    fn info(&self, project: &Project, mounted: &Project, network: &str) {
        self.write(project, "info", &json!({
            "apiVersion":"brig.sh/v1alpha1", "kind":"Info", "data":{
                "profile":project.profile,"session":project.label,"project":{"host":mounted.path},"network":network
            }
        }).to_string());
    }

    fn brig(&self) -> Brig {
        Brig {
            executable: self.0.join("brig"),
            environment: Default::default(),
        }
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.0.join("calls")).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn reads_only_the_verified_full_reference_and_never_the_host_gateway() {
    let fixture = Fixture::new();
    let mut registry = Registry::default();
    let first = fixture.project(&mut registry, "first");
    let second = fixture.project(&mut registry, "second");
    for (project, port) in [(&first, 12345), (&second, 23456)] {
        let project = registry
            .environment(&project.id, &project.environment_id)
            .unwrap();
        fixture.write(&project, "log", &record(&published(port)));
        assert_eq!(
            read_registered(fixture.brig(), &project),
            GatewayLogs::Available {
                text: format!("{TIME} info {}", published(port)),
            }
        );
    }
    fixture.info(&first, &second, "isolated");
    assert_eq!(
        read_registered(fixture.brig(), &first),
        GatewayLogs::Unavailable
    );
    let first = first.reference();
    let second = second.reference();
    assert_eq!(
        fixture.calls(),
        format!(
            concat!(
                "--json info {0}\nlogs --gateway {0} --tail 100\n",
                "--json info {1}\nlogs --gateway {1} --tail 100\n--json info {0}\n"
            ),
            first, second
        )
    );
}

#[test]
fn distinguishes_missing_empty_offline_and_overflow_without_a_fallback() {
    let fixture = Fixture::new();
    let project = fixture.project(&mut Registry::default(), "project");
    assert_eq!(
        read_registered(fixture.brig(), &project),
        GatewayLogs::Unavailable
    );
    fixture.info(
        &project,
        &project,
        "shared (one network for every sandbox on this host)",
    );
    assert_eq!(
        read_registered(fixture.brig(), &project),
        GatewayLogs::Unavailable
    );
    fixture.info(
        &project,
        &project,
        "isolated (a network of this sandbox's own)",
    );
    fixture.write(&project, "log", "\r\n\t\x1b[0m\u{202e}");
    assert_eq!(
        read_registered(fixture.brig(), &project),
        GatewayLogs::Empty
    );
    fixture.write(&project, "overflow", "");
    assert_eq!(
        read_registered(fixture.brig(), &project),
        GatewayLogs::OutputLimit
    );
    fixture.info(
        &project,
        &project,
        "offline (no egress); isolated from its next boot",
    );
    assert_eq!(
        read_registered(fixture.brig(), &project),
        GatewayLogs::Offline
    );
    let calls = fixture.calls();
    let reference = project.reference();
    let commands = format!("--json info {reference}\nlogs --gateway {reference} --tail 100\n");
    assert_eq!(
        calls,
        commands.repeat(4) + &format!("--json info {reference}\n")
    );
}

#[test]
fn recorded_offline_setting_does_not_override_an_observed_isolated_gateway() {
    let fixture = Fixture::new();
    let mut project = fixture.project(&mut Registry::default(), "project");
    project.network = settings::Network::Offline;
    let mut brig = fixture.brig();
    brig.environment
        .insert("BRIG_NETWORK".into(), "offline".into());
    assert_eq!(
        read_registered(brig.clone(), &project),
        GatewayLogs::Unavailable
    );
    fixture.write(&project, "log", &record(&published(12345)));
    assert_eq!(
        read_registered(brig, &project),
        GatewayLogs::Available {
            text: format!("{TIME} info {}", published(12345)),
        }
    );
}

#[test]
fn partial_output_and_stderr_do_not_cross_the_boundary_at_timeout_or_failure() {
    let mut output = CommandOutput {
        stdout: record(&published(12345)),
        stderr: "Authorization: Bearer fixture-secret".into(),
        exit_code: Some(0),
        cancelled: false,
        timed_out: true,
        output_exceeded: false,
    };
    assert_eq!(
        log_result(CommandOutput {
            stdout: output.stdout.clone(),
            stderr: output.stderr.clone(),
            ..output
        }),
        GatewayLogs::Timeout
    );
    output.timed_out = false;
    output.exit_code = Some(2);
    assert_eq!(log_result(output), GatewayLogs::Failed);
}

#[test]
fn filters_the_tail_and_the_clipboard_copy_to_the_same_allowed_fields() {
    let mut raw: String = (10000..10104)
        .map(|port| record(&published(port)))
        .collect();
    raw.push_str(
        &record(
            "\x1b[31megress: refused the A query synthetic-host.invalid. from 10.87.0.2\x1b[0m",
        )
        .replace('\n', "\r\n"),
    );
    raw.push_str("SYNTHETIC-KEY-CONTINUATION\n-----END PRIVATE KEY-----\n");
    raw.push_str(
        "Authorization: Bearer a-synthetic-credential\ntrace gho_syntheticToken\u{202e}\0\n",
    );
    let sanitized = summary::redact(&raw).unwrap();
    let lines: Vec<_> = sanitized.lines().collect();
    assert_eq!(lines.len(), 100);
    assert_eq!(lines[0], format!("{TIME} info {}", published(10009)));
    assert_eq!(
        lines[95],
        format!("{TIME} info egress: refused a DNS query")
    );
    assert_eq!(&lines[96..], &[summary::HIDDEN_ENTRY; 4]);
    assert!(summary::can_copy(&sanitized));
    assert!(!summary::can_copy(&raw));
    assert!(!summary::can_copy(&format!(
        "{TIME} info {}\u{202e}",
        published(10009)
    )));
    assert!(!summary::can_copy(&format!(
        "{TIME} info egress: refused the A query credential"
    )));
    assert!(!summary::can_copy(&format!(
        "{sanitized}\n{}",
        summary::HIDDEN_ENTRY
    )));
}
