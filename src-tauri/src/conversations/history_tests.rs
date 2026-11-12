use super::*;
use serde_json::json;
use std::io::Write;

const CHAT: &str = "cccccccc-0000-4000-8000-000000000001";
const TURN: &str = "aaaaaaaa-0000-4000-8000-000000000001";

fn watch(name: &str, guest: &str) -> Watch {
    let root = std::env::temp_dir().join(format!("tugboat-history-{name}-{}", std::process::id()));
    Watch {
        home: root.join(guest),
        project: "/work/example".into(),
        ledger: root.join("ledger.json"),
    }
}

fn header(provider: &str) -> Vec<Value> {
    vec![
        json!({"type":"session_meta","payload":{"id":CHAT,"cwd":"/work/example","source":"cli","cli_version":"0.167.0","model_provider":provider}}),
        json!({"type":"turn_context","payload":{"turn_id":TURN,"model":"gpt-6.1-sol","root_turn_id":TURN}}),
    ]
}

fn request_pair(i: u64) -> [Value; 2] {
    let tokens = |n: u64| json!({"input_tokens":100*n,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":10*n,"total_tokens":110*n});
    let at = format!("event-{i}");
    [
        json!({"timestamp":at,"type":"token_usage_record","payload":{"thread_id":CHAT,"turn_id":TURN,"root_turn_id":TURN,"response_id":format!("response-{i}"),"usage":tokens(1),"thread_token_usage":tokens(i)}}),
        json!({"timestamp":at,"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":tokens(1),"total_token_usage":tokens(i)}}}),
    ]
}

fn write_history(watch: &Watch, docs: impl IntoIterator<Item = Value>) -> PathBuf {
    let dir = watch.home.join(".codex/sessions");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rollout-example.jsonl");
    let mut file = fs::File::create(&path).unwrap();
    for doc in docs {
        writeln!(file, "{doc}").unwrap();
    }
    path
}

#[test]
fn identical_chat_ids_keep_guest_state_usage_and_details_separate() {
    let a = watch("same-id", "a");
    let b = watch("same-id", "b");
    for guest in [&a, &b] {
        write_history(guest, header("openai").into_iter().chain(request_pair(1)));
    }
    synchronize(&a).unwrap();
    change_state(&a, CHAT, SessionState::Archived, |_| false).unwrap();
    for guest in [&b, &a, &b] {
        let dashboard = synchronize(guest).unwrap().0;
        assert_eq!(dashboard.usage.total, 220);
        assert_eq!(dashboard.conversations.len(), 1);
        assert_eq!(dashboard.conversations[0].usage.total, 110);
        assert_eq!(
            dashboard.conversations[0].state,
            if guest.home == a.home {
                SessionState::Archived
            } else {
                SessionState::Active
            }
        );
        let ledger = guest.selected_ledger().unwrap();
        assert_eq!(
            usage_details::from_ledger(&ledger, CHAT)
                .unwrap()
                .combined_usage
                .total,
            110
        );
        assert!(resume_dashboard(guest, CHAT).is_ok());
    }
    fs::remove_dir_all(a.ledger.parent().unwrap()).unwrap();
}

#[test]
fn same_id_outside_another_guests_project_cannot_exclude_saved_usage() {
    let a = watch("outside-collision", "a");
    let b = watch("outside-collision", "b");
    write_history(&a, header("openai").into_iter().chain(request_pair(1)));
    synchronize(&a).unwrap();
    let mut other = header("openai");
    other[0]["payload"]["cwd"] = "/another/project".into();
    write_history(&b, other.into_iter().chain(request_pair(1)));
    let dashboard = synchronize(&b).unwrap().0;
    assert_eq!(dashboard.usage.total, 110);
    assert!(!dashboard.conversations[0].available);
    assert!(resume_dashboard(&b, CHAT).is_err());
    assert!(resume_dashboard(&a, CHAT).is_ok());
    fs::remove_dir_all(a.ledger.parent().unwrap()).unwrap();
}

#[test]
fn mixed_legacy_rows_keep_unattributed_spending_out_of_both_guest_details() {
    for version in [1, 2] {
        let a = watch(&format!("mixed-v{version}"), "a");
        let b = watch(&format!("mixed-v{version}"), "b");
        for guest in [&a, &b] {
            write_history(guest, header("openai").into_iter().chain(request_pair(1)));
            synchronize(guest).unwrap();
        }
        let mut old = load_ledger(&a.ledger).unwrap();
        let mut other = old
            .variants
            .remove(CHAT)
            .unwrap()
            .remove(&a.namespace())
            .unwrap();
        let row = old.sessions.get_mut(CHAT).unwrap();
        row.guest_namespace = None;
        row.cursors.append(&mut other.cursors);
        row.state = SessionState::Archived;
        let requests = &mut row.turns.get_mut(TURN).unwrap().requests;
        requests.append(&mut other.turns.get_mut(TURN).unwrap().requests);
        let mut historical = requests[0].clone();
        historical.key = "legacy-unattributed".into();
        historical.source = None;
        historical.cost_usd = Some(1.25);
        requests.push(historical);
        old.schema_version = version;
        storage::write(&a.ledger, &old).unwrap();
        for guest in [&a, &b, &a] {
            let dashboard = synchronize(guest).unwrap().0;
            assert_eq!(dashboard.usage.total, 330);
            assert_eq!(dashboard.conversations[0].state, SessionState::Active);
            let selected = guest.selected_ledger().unwrap();
            assert_eq!(
                usage_details::from_ledger(&selected, CHAT)
                    .unwrap()
                    .combined_usage
                    .total,
                110
            );
            let retained = &selected.variants[CHAT][""];
            assert_eq!(retained.state, SessionState::Archived);
            assert_eq!(retained.turns[TURN].requests[0].cost_usd, Some(1.25));
        }
        fs::remove_dir_all(a.ledger.parent().unwrap()).unwrap();
    }
}

#[test]
fn oversized_unfinished_line_does_not_hide_the_next_request() {
    let w = watch("oversized", "home");
    let path = write_history(&w, header("openai").into_iter().chain(request_pair(1)));
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(&vec![b'x'; 17 * 1024 * 1024]).unwrap();
    assert_eq!(synchronize(&w).unwrap().0.usage.requests, 1);
    writeln!(file).unwrap();
    for doc in request_pair(2) {
        writeln!(file, "{doc}").unwrap();
    }
    for _ in 0..2 {
        let dashboard = synchronize(&w).unwrap().0;
        assert_eq!(dashboard.usage.requests, 2);
        assert!(dashboard.conversations[0]
            .warning
            .as_deref()
            .unwrap()
            .contains("oversized"));
    }
    fs::remove_dir_all(w.ledger.parent().unwrap()).unwrap();
}

#[test]
fn provider_price_is_independent_of_record_order_and_repaired_offline() {
    for counter_first in [true, false] {
        let w = watch(&format!("provider-{counter_first}"), "home");
        let mut pair = request_pair(1);
        if counter_first {
            pair.reverse();
        }
        let path = write_history(&w, header("custom").into_iter().chain(pair));
        for _ in 0..2 {
            let d = synchronize(&w).unwrap().0;
            assert_eq!(
                (
                    d.usage.requests,
                    d.usage.total,
                    d.usage.unpriced_requests,
                    d.usage.cost_usd
                ),
                (1, 110, 1, 0.0)
            );
        }
        if counter_first {
            let mut old = serde_json::to_value(load_ledger(&w.ledger).unwrap()).unwrap();
            let req = &mut old["sessions"][CHAT]["turns"][TURN]["requests"][0];
            req["rates"] = req["providerCorrection"]["previousRates"].clone();
            req["costUsd"] = 0.0003.into();
            req.as_object_mut().unwrap().remove("providerCorrection");
            storage::write(&w.ledger, &old).unwrap();
            fs::remove_file(path).unwrap();
            assert_eq!(synchronize(&w).unwrap().0.usage.unpriced_requests, 1);
            let saved = serde_json::to_value(load_ledger(&w.ledger).unwrap()).unwrap();
            assert_eq!(
                saved["sessions"][CHAT]["turns"][TURN]["requests"][0]["providerCorrection"]
                    ["previousCostUsd"],
                0.0003
            );
        }
        fs::remove_dir_all(w.ledger.parent().unwrap()).unwrap();
    }
}

#[test]
fn unreadable_branch_keeps_healthy_and_saved_usage_and_recovers() {
    use std::os::unix::fs::PermissionsExt;
    let w = watch("unreadable", "home");
    let path = write_history(&w, header("openai").into_iter().chain(request_pair(1)));
    let dir = path.parent().unwrap().join("2026");
    fs::create_dir(&dir).unwrap();
    let second = dir.join("rollout-second.jsonl");
    fs::write(
        &second,
        fs::read_to_string(path)
            .unwrap()
            .replace(CHAT, "cccccccc-0000-4000-8000-000000000002"),
    )
    .unwrap();
    assert_eq!(synchronize(&w).unwrap().0.usage.requests, 2);
    let mut file = fs::OpenOptions::new().append(true).open(&second).unwrap();
    for doc in request_pair(2) {
        writeln!(
            file,
            "{}",
            doc.to_string()
                .replace(CHAT, "cccccccc-0000-4000-8000-000000000002")
        )
        .unwrap();
    }
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
    let partial = synchronize(&w);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let partial = partial.unwrap().0;
    assert_eq!(partial.usage.requests, 2);
    assert!(!partial.warnings.is_empty());
    assert!(partial
        .conversations
        .iter()
        .any(|row| row.id == CHAT && row.available));
    assert!(partial.conversations.iter().any(|row| !row.available
        && row
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("directory"))));
    for _ in 0..2 {
        let d = synchronize(&w).unwrap().0;
        assert_eq!(d.usage.requests, 3);
        assert!(d.warnings.is_empty());
    }
    fs::remove_dir_all(w.ledger.parent().unwrap()).unwrap();
}

#[test]
fn read_budget_reports_partial_coverage_until_the_next_pass() {
    let w = watch("read-budget", "home");
    let path = write_history(&w, header("openai").into_iter().chain(request_pair(1)));
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    let output =
        json!({"type":"response_item","payload":{"output":"a".repeat(1024*1024)}}).to_string();
    for _ in 0..65 {
        writeln!(file, "{output}").unwrap();
    }
    for doc in request_pair(2) {
        writeln!(file, "{doc}").unwrap();
    }
    let first = synchronize(&w).unwrap().0;
    assert_eq!(
        (first.usage.requests, first.coverage),
        (1, usage::Coverage::Partial)
    );
    let ledger = w.selected_ledger().unwrap();
    assert_eq!(
        usage_details::from_ledger(&ledger, CHAT).unwrap().coverage,
        usage::Coverage::Partial
    );
    let second = synchronize(&w).unwrap().0;
    assert_eq!(
        (second.usage.requests, second.coverage),
        (2, usage::Coverage::Complete)
    );
    fs::remove_dir_all(w.ledger.parent().unwrap()).unwrap();
}

#[test]
fn matched_notifications_do_not_exhaust_the_pending_queue() {
    for n in [256, 257, 300] {
        for counter_first in [true, false] {
            let w = watch(&format!("pairs-{n}-{counter_first}"), "home");
            write_history(
                &w,
                header("openai").into_iter().chain((1..=n).flat_map(|i| {
                    let mut pair = request_pair(i);
                    if counter_first {
                        pair.reverse();
                    }
                    pair
                })),
            );
            for _ in 0..2 {
                let d = synchronize(&w).unwrap().0;
                assert_eq!(
                    (d.usage.requests, d.usage.total, d.coverage),
                    (n, 110 * n, usage::Coverage::Complete)
                );
            }
            fs::remove_dir_all(w.ledger.parent().unwrap()).unwrap();
        }
    }
}

#[test]
fn long_history_import_counts_each_indexed_request_once() {
    for n in [250, 500, 1000] {
        let w = watch(&format!("long-{n}"), "home");
        write_history(
            &w,
            header("openai")
                .into_iter()
                .chain((1..=n).flat_map(request_pair)),
        );
        let start = std::time::Instant::now();
        let d = synchronize(&w).unwrap().0;
        println!("{n} requests: {} ms", start.elapsed().as_millis());
        assert_eq!(d.usage.requests, n);
        assert_eq!(synchronize(&w).unwrap().0.usage.total, 110 * n);
        fs::remove_dir_all(w.ledger.parent().unwrap()).unwrap();
    }
}
