//! The smallest launcher plugin: a counter kept in its storage, a timer, and the test
//! hooks the launcher's own tests use (`src/core/plugin_host/tests.rs`): buttons that
//! loop forever, use too much memory, copy, or ask for what the plugin may not do.
use echovr_plugin::*;

#[derive(Default)]
struct S {
    count: u32,
    ticks: u32,
    note: String,
}

static STATE: State<S> = State::new();

struct Example;

impl Plugin for Example {
    fn init(_info: &Value) -> Result<(), String> {
        let count = host::storage_get("count")
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        STATE.with(|s| s.count = count);
        host::set_timer(1, 1000, true);
        Ok(())
    }

    fn render(ui: &mut Ui) {
        STATE.with(|s| {
            ui.card(0, "Example", "A plugin as small as it gets.", |c| {
                c.value("Clicked", s.count.to_string(), false);
                c.value("Seconds open", s.ticks.to_string(), false);
                c.buttons(vec![
                    button("add", "ADD ONE").primary(),
                    button("copy", "COPY"),
                    button("fetch", "FETCH"),
                ]);
                if !s.note.is_empty() {
                    c.muted(s.note.clone());
                }
            });
        })
    }

    fn on_event(e: Event) {
        match e {
            Event::Clicked(id) => match id.as_str() {
                "add" => STATE.with(|s| {
                    s.count += 1;
                    let _ = host::storage_set("count", &s.count.to_string());
                }),
                "copy" => {
                    let r = host::copy("example");
                    STATE.with(|s| s.note = format!("copy: {r:?}"));
                }
                "fetch" => {
                    let r = get("https://example.org/");
                    STATE.with(|s| s.note = format!("fetch: {r:?}"));
                }
                // Test hooks.
                "loop" => loop {
                    std::hint::black_box(());
                },
                "grow" => {
                    let mut v: Vec<Vec<u8>> = Vec::new();
                    loop {
                        v.push(vec![1u8; 1 << 20]);
                        std::hint::black_box(&v);
                    }
                }
                "panic" => panic!("asked to"),
                "host" => {
                    let r = call("config", &json!({}));
                    STATE.with(|s| s.note = format!("server-host: {r:?}"));
                }
                _ => {}
            },
            Event::NetDone((t, r)) => STATE.with(|s| {
                s.note = match r {
                    Ok(resp) => format!("ticket {t}: {} ({} bytes)", resp.status, resp.body.len()),
                    Err(e) => format!("ticket {t}: {e}"),
                }
            }),
            _ => {}
        }
        host::invalidate();
    }

    fn on_timer(_id: u32) {
        STATE.with(|s| s.ticks += 1);
        host::invalidate();
    }
}

export_plugin!(Example);
