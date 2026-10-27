//! The sandbox, with the example plugin (`plugins/example`, built into `testdata/` by
//! `scripts/build-plugins.sh`).

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use super::*;
use crate::core::launcher::plugins;

const EXAMPLE: &[u8] = include_bytes!("testdata/example.wasm");

/// The example plugin in a folder of its own, its `plugin.json` declaring `permissions`.
fn example_in(dir: &Path, permissions: Value) -> Spec {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("plugin.wasm"), EXAMPLE).unwrap();
    let manifest = json!({
        "schema": 2, "id": "example", "name": "Example", "version": "0.1.0",
        "wasm": "plugin.wasm", "permissions": permissions,
    });
    std::fs::write(dir.join("plugin.json"), manifest.to_string()).unwrap();
    let (page, w) = plugins::read_description(&manifest.to_string());
    assert!(w.is_empty(), "{w:?}");
    Spec {
        page: Arc::new(page.unwrap()),
        dir: dir.to_path_buf(),
        first_party: false,
    }
}

fn started(spec: Spec, granted: Permissions) -> Running {
    let shared = Shared {
        info: json!({"features": {}}),
        granted,
        ..Default::default()
    };
    spawn(spec, shared, Arc::new(|| {}))
}

/// The next page it draws (within `wait`), skipping other messages.
fn next_page(r: &Running, wait: Duration) -> Option<Page> {
    let until = Instant::now() + wait;
    loop {
        match r
            .rx
            .recv_timeout(until.saturating_duration_since(Instant::now()))
        {
            Ok(Out::Page(p)) => return Some(p),
            Ok(Out::Stopped { why, .. }) => panic!("stopped: {why}"),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

/// Every message until it stops (within `wait`): why, and whether it may start again.
fn stopped(r: &Running, wait: Duration) -> (String, bool) {
    let until = Instant::now() + wait;
    loop {
        match r
            .rx
            .recv_timeout(until.saturating_duration_since(Instant::now()))
        {
            Ok(Out::Stopped { why, restart }) => return (why, restart),
            Ok(_) => {}
            Err(_) => panic!("it didn't stop"),
        }
    }
}

fn value_of(p: &Page, label: &str) -> String {
    p.nodes
        .iter()
        .find_map(|n| match n {
            Node::Value(v) if v.label == label => Some(v.value.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn note(p: &Page) -> String {
    p.nodes
        .iter()
        .find_map(|n| match n {
            Node::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
fn draws_and_answers_clicks() {
    let tmp = tempfile::tempdir().unwrap();
    let r = started(example_in(tmp.path(), Value::Null), Permissions::default());
    let first = next_page(&r, Duration::from_secs(20)).expect("its first page");
    assert_eq!(value_of(&first, "Clicked"), "0");
    assert!(matches!(first.nodes.last(), Some(Node::Card(c)) if c.title == "Example"));
    r.event(UiEvent::Clicked("add".into()));
    let after = next_page(&r, Duration::from_secs(5)).unwrap();
    assert_eq!(value_of(&after, "Clicked"), "1");
    // Kept in its own storage, a dot folder (not one of its own files).
    let kept = std::fs::read_to_string(tmp.path().join(imports::STORAGE)).unwrap();
    assert!(kept.contains("\"count\":\"1\""), "{kept}");
}

#[test]
fn timers_fire() {
    let tmp = tempfile::tempdir().unwrap();
    let r = started(example_in(tmp.path(), Value::Null), Permissions::default());
    next_page(&r, Duration::from_secs(20)).unwrap();
    let ticked = next_page(&r, Duration::from_secs(5)).expect("a page after its timer");
    assert_eq!(value_of(&ticked, "Seconds open"), "1");
}

#[test]
fn an_endless_loop_is_stopped_and_it_can_start_again() {
    let tmp = tempfile::tempdir().unwrap();
    let r = started(example_in(tmp.path(), Value::Null), Permissions::default());
    next_page(&r, Duration::from_secs(20)).unwrap();
    let t = Instant::now();
    r.event(UiEvent::Clicked("loop".into()));
    let (why, restart) = stopped(&r, Duration::from_secs(10));
    assert!(why.contains("too long"), "{why}");
    assert!(restart);
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    // The engine is still good: a new instance draws again.
    r.send(Cmd::Restart);
    assert!(next_page(&r, Duration::from_secs(10)).is_some());
}

#[test]
fn too_much_memory_stops_it() {
    let tmp = tempfile::tempdir().unwrap();
    let r = started(example_in(tmp.path(), Value::Null), Permissions::default());
    next_page(&r, Duration::from_secs(20)).unwrap();
    r.event(UiEvent::Clicked("grow".into()));
    let (why, _) = stopped(&r, Duration::from_secs(10));
    assert!(why.contains("too much memory"), "{why}");
}

#[test]
fn a_panic_stops_it_three_times_at_most() {
    let tmp = tempfile::tempdir().unwrap();
    let r = started(example_in(tmp.path(), Value::Null), Permissions::default());
    for i in 0..=RESTARTS {
        next_page(&r, Duration::from_secs(20)).unwrap();
        r.event(UiEvent::Clicked("panic".into()));
        let (why, restart) = stopped(&r, Duration::from_secs(10));
        assert!(why.contains("crashed"), "{why}");
        assert_eq!(restart, i < RESTARTS, "after {i} restarts");
        r.send(Cmd::Restart);
    }
}

#[test]
fn requests_go_only_where_allowed() {
    let tmp = tempfile::tempdir().unwrap();
    // Declared but not allowed: refused.
    let declared = json!({"net": ["https://example.org"]});
    let r = started(
        example_in(tmp.path(), declared.clone()),
        Permissions::default(),
    );
    next_page(&r, Duration::from_secs(20)).unwrap();
    r.event(UiEvent::Clicked("fetch".into()));
    let p = next_page(&r, Duration::from_secs(5)).unwrap();
    assert!(
        note(&p).contains("may not talk to https://example.org:443"),
        "{}",
        note(&p)
    );
    drop(r);
    // Allowed but not declared: refused too.
    let tmp2 = tempfile::tempdir().unwrap();
    let mut granted = Permissions::default();
    granted.servers.insert("https://example.org:443".into());
    let r = started(example_in(tmp2.path(), Value::Null), granted.clone());
    next_page(&r, Duration::from_secs(20)).unwrap();
    r.event(UiEvent::Clicked("fetch".into()));
    let p = next_page(&r, Duration::from_secs(5)).unwrap();
    assert!(note(&p).contains("may not talk to"), "{}", note(&p));
}

#[test]
fn services_need_permission_and_a_signature() {
    let tmp = tempfile::tempdir().unwrap();
    let r = started(example_in(tmp.path(), Value::Null), Permissions::default());
    next_page(&r, Duration::from_secs(20)).unwrap();
    r.event(UiEvent::Clicked("host".into()));
    let p = next_page(&r, Duration::from_secs(5)).unwrap();
    assert!(note(&p).contains("may not use server-host"), "{}", note(&p));
}

#[test]
fn copying_needs_a_click() {
    let tmp = tempfile::tempdir().unwrap();
    let r = started(example_in(tmp.path(), Value::Null), Permissions::default());
    next_page(&r, Duration::from_secs(20)).unwrap();
    r.event(UiEvent::Clicked("copy".into()));
    let until = Instant::now() + Duration::from_secs(5);
    let mut copied = false;
    while let Ok(o) =
        r.rx.recv_timeout(until.saturating_duration_since(Instant::now()))
    {
        match o {
            Out::Copy(t) => {
                assert_eq!(t, "example");
                copied = true;
            }
            Out::Page(_) if copied => break,
            _ => {}
        }
    }
    assert!(copied);
}

#[test]
fn the_compiled_component_is_kept_and_checked() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = example_in(tmp.path(), Value::Null);
    let wasm = tmp.path().join(&spec.page.wasm);
    component(&wasm).unwrap();
    let kept: Vec<_> = std::fs::read_dir(tmp.path().join(".compiled"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(kept.iter().any(|n| n.ends_with(".cwasm")), "{kept:?}");
    // A changed cache file is compiled again, not loaded.
    let cwasm = kept.iter().find(|n| n.ends_with(".cwasm")).unwrap();
    std::fs::write(tmp.path().join(".compiled").join(cwasm), b"not a component").unwrap();
    assert!(component(&wasm).is_ok());
}

#[test]
fn checked_pages_drop_what_points_nowhere() {
    let page = Page {
        nodes: vec![Node::Card(types::Card {
            title: "x".repeat(MAX_TEXT + 10),
            help: String::new(),
            children: vec![0, 7],
        })],
        columns: vec![vec![0, 3], vec![], vec![0]],
    };
    let p = checked(page);
    assert_eq!(p.columns, vec![vec![0], vec![]]);
    match &p.nodes[0] {
        Node::Card(c) => {
            assert_eq!(c.children, vec![0]);
            assert_eq!(c.title.len(), MAX_TEXT);
        }
        _ => unreachable!(),
    }
}

#[test]
fn the_relay_account_is_filled_in_by_the_launcher() {
    let body = json!({"displayname": "{relay.name}", "password": "{relay.password}", "x": ["{relay.password}"]});
    let out: Value =
        serde_json::from_str(&imports::with_account(&body.to_string(), "mia", "pw")).unwrap();
    assert_eq!(
        out,
        json!({"displayname": "mia", "password": "pw", "x": ["pw"]})
    );
}
