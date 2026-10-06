//! The animated background: the designer's loop (`assets/video/background.h264`, raw
//! H.264 at 1280×720 and `FPS`), decoded on a thread and shown as one texture, updated
//! as its frames come due. `main_background.jpg` is its first frame, drawn until the
//! first one is decoded.

use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};

use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;

const BYTES: &[u8] = include_bytes!("../../assets/video/background.h264");
const FPS: f64 = 30.0;
/// Decoded frames kept ready.
const AHEAD: usize = 4;

#[derive(Default)]
pub struct BackgroundVideo {
    /// Decoded frames, `AHEAD` of the one shown: the decoder waits (idle) while this is
    /// full.
    frames: Option<Receiver<ColorImage>>,
    texture: Option<TextureHandle>,
    /// When the frame on screen came due (`egui::InputState::time`).
    shown_at: f64,
}

impl BackgroundVideo {
    /// The frame to draw, moving on to the next one when it is due and `play` is set
    /// (else the picture holds, and so does the decoder). `None` until the first frame is
    /// decoded.
    pub fn frame(&mut self, ctx: &Context, play: bool) -> Option<&TextureHandle> {
        let frames = self.frames.get_or_insert_with(start);
        let now = ctx.input(|i| i.time);
        let dt = 1.0 / FPS;
        if self.texture.is_none() || (play && now - self.shown_at >= dt) {
            // Every frame that came due since the last one shown: more than one when the
            // screen refreshes slower than the video plays.
            let due = if self.texture.is_none() {
                1
            } else {
                (((now - self.shown_at) / dt).floor() as usize).clamp(1, AHEAD)
            };
            let mut latest = None;
            let mut taken = 0;
            while taken < due {
                match frames.try_recv() {
                    Ok(img) => {
                        latest = Some(img);
                        taken += 1;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        tracing::warn!("background video stopped");
                        self.frames = None;
                        self.texture = None;
                        return None;
                    }
                }
            }
            if let Some(img) = latest {
                match &mut self.texture {
                    Some(t) => t.set(img, TextureOptions::LINEAR),
                    None => {
                        self.texture =
                            Some(ctx.load_texture("background-video", img, TextureOptions::LINEAR))
                    }
                }
                // Keep the beat, unless the picture held or the decoder fell behind.
                let next = self.shown_at + taken as f64 * dt;
                self.shown_at = if now - next < dt { next } else { now };
            }
        }
        if play || self.texture.is_none() {
            let wait = (self.shown_at + dt - now).clamp(0.005, dt);
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
        self.texture.as_ref()
    }
}

/// The decoder thread: the loop over and over, until the receiver is dropped (or the
/// stream has no pictures).
fn start() -> Receiver<ColorImage> {
    let (tx, rx) = sync_channel(AHEAD);
    let spawned = std::thread::Builder::new()
        .name("background-video".into())
        .spawn(move || loop {
            let bytes = BYTES;
            let mut decoded = 0;
            let mut decoder = match Decoder::new() {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("background video: no decoder: {e}");
                    return;
                }
            };
            let mut rgba = Vec::new();
            for nal in openh264::nal_units(bytes) {
                match decoder.decode(nal) {
                    Ok(Some(yuv)) => {
                        decoded += 1;
                        let (w, h) = yuv.dimensions();
                        rgba.resize(w * h * 4, 0);
                        yuv.write_rgba8(&mut rgba);
                        let img = ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                        if tx.send(img).is_err() {
                            return;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => tracing::debug!("background video: {e}"),
                }
            }
            if decoded == 0 {
                tracing::warn!("background video: no pictures in the stream");
                return;
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("background video: no thread: {e}");
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_loop() {
        let mut decoder = Decoder::new().unwrap();
        let mut frames = 0;
        for nal in openh264::nal_units(BYTES) {
            if let Some(yuv) = decoder.decode(nal).unwrap() {
                assert_eq!(yuv.dimensions(), (1280, 720));
                frames += 1;
                if frames == 3 {
                    return;
                }
            }
        }
        panic!("only {frames} frames");
    }
}
