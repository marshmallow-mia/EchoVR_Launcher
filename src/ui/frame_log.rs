//! `ECHOVR_FRAME_LOG=1`: once a second, how many frames the window drew and what asked
//! for them (egui's repaint causes, by file and line). For checking that an idle launcher
//! draws nothing.

use std::collections::BTreeMap;

#[derive(Default)]
pub struct FrameLog {
    on: Option<bool>,
    since: f64,
    frames: u32,
    causes: BTreeMap<String, u32>,
}

impl FrameLog {
    pub fn frame(&mut self, ctx: &egui::Context) {
        if !*self
            .on
            .get_or_insert_with(|| std::env::var_os("ECHOVR_FRAME_LOG").is_some_and(|v| v == "1"))
        {
            return;
        }
        self.frames += 1;
        // What counted as input, by kind only (never what was typed).
        let input: Vec<String> = ctx.input(|i| {
            let mut kinds: Vec<String> = i
                .events
                .iter()
                .map(|e| {
                    let d = format!("{e:?}");
                    d.split(['(', ' ', '{'])
                        .next()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            if i.pointer.is_moving() {
                kinds.push("pointer-moving".into());
            }
            if i.pointer.any_down() {
                kinds.push("pointer-down".into());
            }
            kinds
        });
        for k in input {
            *self.causes.entry(format!("input:{k}")).or_default() += 1;
        }
        let (events, moving, down) = ctx.input(|i| {
            (
                i.events
                    .iter()
                    .map(|e| format!("{e:?}").chars().take(60).collect::<String>())
                    .collect::<Vec<_>>(),
                i.pointer.is_moving(),
                i.pointer.any_down(),
            )
        });
        if !events.is_empty() || moving || down {
            *self
                .causes
                .entry(format!("INPUT moving={moving} down={down} {events:?}"))
                .or_default() += 1;
        }
        for c in ctx.repaint_causes() {
            *self
                .causes
                .entry(format!("{}:{}", c.file, c.line))
                .or_default() += 1;
        }
        let now = ctx.input(|i| i.time);
        if now - self.since >= 1.0 {
            let causes: Vec<String> = std::mem::take(&mut self.causes)
                .into_iter()
                .map(|(k, n)| format!("{k}×{n}"))
                .collect();
            tracing::info!(
                "frames: {} in {:.1} s; causes: {}",
                self.frames,
                now - self.since,
                causes.join(", ")
            );
            self.frames = 0;
            self.since = now;
        }
    }
}
