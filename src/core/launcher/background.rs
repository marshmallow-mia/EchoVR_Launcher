//! A custom background (Settings): the user's video or picture, converted to what the
//! launcher plays, in `<data dir>/background`:
//!
//! * `background.h264`: raw H.264 (Constrained Baseline, Annex B) at 1280×720 and 24
//!   fps, like the built-in `assets/video/background.h264`, at most `MAX_SECONDS` long;
//! * `still.jpg`: its first frame (or the picture) at 1920×1080, shown until the video
//!   runs and whenever the animation is off;
//! * `name.txt`: the original file's name.
//!
//! Both are cropped to fill the window. Videos need ffmpeg (`core::ffmpeg`); pictures
//! (PNG, JPEG) don't.

use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};

use super::versions::Step;
use crate::core::paths;

/// Longer videos are cut here (the loop restarts after it).
pub const MAX_SECONDS: u32 = 60;
const VIDEO: &str = "background.h264";
const STILL: &str = "still.jpg";
const NAME: &str = "name.txt";
/// The file dialog's choices.
pub const VIDEO_EXTENSIONS: [&str; 7] = ["mp4", "mov", "m4v", "webm", "mkv", "avi", "gif"];
pub const PICTURE_EXTENSIONS: [&str; 3] = ["png", "jpg", "jpeg"];

pub fn dir() -> PathBuf {
    paths::data_dir().join("background")
}

/// The custom background in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Custom {
    /// The original file's name.
    pub name: String,
    pub still: PathBuf,
    /// `None` for a picture.
    pub video: Option<PathBuf>,
}

pub fn current() -> Option<Custom> {
    let dir = dir();
    let still = dir.join(STILL);
    if !still.is_file() {
        return None;
    }
    let video = Some(dir.join(VIDEO)).filter(|p| p.is_file());
    let name = std::fs::read_to_string(dir.join(NAME))
        .map(|n| n.trim().to_string())
        .unwrap_or_default();
    Some(Custom { name, still, video })
}

/// Back to the built-in background.
pub fn reset() -> Result<()> {
    let dir = dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("delete {}", dir.display()))?;
    }
    Ok(())
}

/// A picture (converted without ffmpeg), by its extension.
pub fn is_picture(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| PICTURE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Converts `input` and makes it the background. Videos need `ffmpeg`.
pub fn convert(
    input: &Path,
    ffmpeg: Option<&Path>,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<()> {
    convert_to(&dir(), input, ffmpeg, cancel, on)
}

/// `convert` into `target`, replacing what is there only once the new one is complete.
fn convert_to(
    target: &Path,
    input: &Path,
    ffmpeg: Option<&Path>,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<()> {
    let staging = target.with_extension("new");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging).with_context(|| format!("create {}", staging.display()))?;
    let made = if is_picture(input) {
        on(Step::Status("Converting the picture...".into()));
        let img = image::open(input).context("This picture can't be read")?;
        save_still(&img, &staging.join(STILL))
    } else {
        let ffmpeg = ffmpeg.context("Converting a video needs ffmpeg")?;
        video(ffmpeg, input, &staging, cancel, on)
    };
    if let Err(e) = made {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    let name = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    std::fs::write(staging.join(NAME), name)?;
    if target.exists() {
        std::fs::remove_dir_all(target).with_context(|| format!("delete {}", target.display()))?;
    }
    std::fs::rename(&staging, target)
        .with_context(|| format!("move {} into place", target.display()))?;
    tracing::info!("custom background from {}", input.display());
    Ok(())
}

/// The picture cropped to fill 1920×1080, as a JPEG.
fn save_still(img: &image::DynamicImage, out: &Path) -> Result<()> {
    let (w, h) = (1920, 1080);
    let s = (w as f32 / img.width() as f32).max(h as f32 / img.height() as f32);
    let (rw, rh) = (
        ((img.width() as f32 * s).ceil() as u32).max(w),
        ((img.height() as f32 * s).ceil() as u32).max(h),
    );
    let resized = img
        .resize_exact(rw, rh, image::imageops::FilterType::Lanczos3)
        .to_rgb8();
    let cropped = image::imageops::crop_imm(&resized, (rw - w) / 2, (rh - h) / 2, w, h).to_image();
    let mut file = BufWriter::new(File::create(out)?);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut file, 88)
        .encode_image(&cropped)
        .context("saving the picture")?;
    Ok(())
}

/// The video's first `MAX_SECONDS` at 1280×720 and 24 fps, and its first frame.
fn video(
    ffmpeg: &Path,
    input: &Path,
    out: &Path,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<()> {
    on(Step::Status("Converting the video...".into()));
    let h264 = out.join(VIDEO);
    let fill =
        |w: u32, h: u32| format!("scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}");
    let mut args: Vec<OsString> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-y".into(),
        "-i".into(),
        input.into(),
        "-t".into(),
        MAX_SECONDS.to_string().into(),
        "-an".into(),
        "-vf".into(),
        format!("{},fps=24,format=yuv420p", fill(1280, 720)).into(),
    ];
    for a in [
        "-c:v",
        "libx264",
        "-profile:v",
        "baseline",
        "-preset",
        "slow",
        "-crf",
        "20",
        "-g",
        "240",
        "-bf",
        "0",
        "-bsf:v",
        "h264_mp4toannexb",
        "-f",
        "h264",
        "-progress",
        "pipe:1",
        "-nostats",
    ] {
        args.push(a.into());
    }
    args.push(h264.clone().into());
    run(ffmpeg, &args, cancel, on)?;
    check_video(&h264)?;

    on(Step::Status("Saving its first frame...".into()));
    let still: Vec<OsString> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-y".into(),
        "-i".into(),
        input.into(),
        "-frames:v".into(),
        "1".into(),
        "-vf".into(),
        fill(1920, 1080).into(),
        "-q:v".into(),
        "3".into(),
        out.join(STILL).into(),
    ];
    run(ffmpeg, &still, cancel, &mut |_| {})
}

/// Runs ffmpeg, reporting its progress (`-progress pipe:1`) against the input's length.
fn run(
    ffmpeg: &Path,
    args: &[OsString],
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Step),
) -> Result<()> {
    let mut child = crate::core::process::command(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("couldn't start {}", ffmpeg.display()))?;
    // stderr: the input's length (for the percentage) and the last lines (for errors).
    let duration = Arc::new(Mutex::new(None::<f64>));
    let tail = Arc::new(Mutex::new(Vec::<String>::new()));
    let stderr = child.stderr.take().context("ffmpeg's stderr")?;
    let reader = {
        let (duration, tail) = (duration.clone(), tail.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(d) = parse_duration(&line) {
                    duration
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get_or_insert(d);
                }
                let mut t = tail.lock().unwrap_or_else(|e| e.into_inner());
                t.push(line);
                if t.len() > 12 {
                    t.remove(0);
                }
            }
        })
    };
    let stdout = child.stdout.take().context("ffmpeg's stdout")?;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(crate::core::http::Cancelled.into());
        }
        let Some(us) = line
            .strip_prefix("out_time_us=")
            .and_then(|v| v.trim().parse::<f64>().ok())
        else {
            continue;
        };
        let total = duration
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map_or(f64::from(MAX_SECONDS), |d| d.min(f64::from(MAX_SECONDS)));
        if total > 0.0 {
            on(Step::Percent((us / 1e6 / total * 100.0).clamp(0.0, 100.0)));
        }
    }
    let status = child.wait()?;
    let _ = reader.join();
    if cancel.load(Ordering::Relaxed) {
        return Err(crate::core::http::Cancelled.into());
    }
    if !status.success() {
        let tail = tail.lock().unwrap_or_else(|e| e.into_inner()).join("\n");
        tracing::warn!("ffmpeg failed: {tail}");
        if tail.contains("libx264") {
            bail!("This ffmpeg can't encode H.264 (libx264 is missing). Install a full ffmpeg.");
        }
        bail!("This file couldn't be converted.\n\n{}", last_line(&tail));
    }
    Ok(())
}

/// `  Duration: 00:01:02.50, start: ...` → 62.5 seconds.
fn parse_duration(line: &str) -> Option<f64> {
    let rest = line.trim_start().strip_prefix("Duration: ")?;
    let hms = rest.split(',').next()?;
    let mut parts = hms.split(':').map(|p| p.trim().parse::<f64>().ok());
    let (h, m, s) = (parts.next()??, parts.next()??, parts.next()??);
    Some(h * 3600.0 + m * 60.0 + s)
}

/// The most telling line of ffmpeg's output: its last non-empty one.
fn last_line(tail: &str) -> &str {
    tail.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("ffmpeg failed")
}

/// The converted stream decodes, at 1280×720.
fn check_video(path: &Path) -> Result<()> {
    use openh264::formats::YUVSource;
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    let mut decoder = openh264::decoder::Decoder::new().context("no H.264 decoder")?;
    for nal in openh264::nal_units(&bytes) {
        if let Ok(Some(frame)) = decoder.decode(nal) {
            if frame.dimensions() != (1280, 720) {
                bail!("The converted video has the wrong size");
            }
            return Ok(());
        }
    }
    bail!("The converted video has no pictures")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_durations() {
        assert_eq!(
            parse_duration("  Duration: 00:01:02.50, start: 0.000000, bitrate: 49578 kb/s"),
            Some(62.5)
        );
        assert_eq!(parse_duration("Stream #0:0: Video: h264"), None);
    }

    #[test]
    fn tells_pictures_from_videos() {
        assert!(is_picture(Path::new("C:/x/Wallpaper.JPG")));
        assert!(is_picture(Path::new("a.png")));
        assert!(!is_picture(Path::new("clip.mp4")));
        assert!(!is_picture(Path::new("anim.gif")));
    }

    #[test]
    fn converts_a_picture() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("Wallpaper.png");
        image::DynamicImage::new_rgb8(2560, 1440)
            .save(&input)
            .unwrap();
        let target = dir.path().join("background");
        let cancel = AtomicBool::new(false);
        convert_to(&target, &input, None, &cancel, &mut |_| {}).unwrap();
        assert!(target.join(STILL).is_file());
        assert!(!target.join(VIDEO).exists());
        assert_eq!(
            std::fs::read_to_string(target.join(NAME)).unwrap(),
            "Wallpaper.png"
        );
        assert!(!target.with_extension("new").exists());
    }

    /// A B-frame H.264 video, as phones and OBS make them (needs ffmpeg on the PATH;
    /// skipped without one).
    #[test]
    fn converts_a_video() {
        let Some(ffmpeg) = crate::core::ffmpeg::find() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("clip.mp4");
        let made = crate::core::process::run(
            &ffmpeg,
            &[
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=1920x1080:rate=30",
                "-t",
                "2",
                "-c:v",
                "libx264",
                "-profile:v",
                "high",
                "-bf",
                "3",
                "-pix_fmt",
                "yuv420p",
            ]
            .iter()
            .map(|s| s.to_string())
            .chain([input.to_string_lossy().into_owned()])
            .collect::<Vec<_>>(),
            None,
        );
        if !made.success() {
            return;
        }
        let target = dir.path().join("background");
        let cancel = AtomicBool::new(false);
        let mut last = 0.0;
        convert_to(&target, &input, Some(&ffmpeg), &cancel, &mut |s| {
            if let Step::Percent(p) = s {
                last = p;
            }
        })
        .unwrap();
        assert!(target.join(VIDEO).is_file() && target.join(STILL).is_file());
        assert!(last > 90.0, "progress reached {last}%");
    }

    #[test]
    fn pictures_fill_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("still.jpg");
        let tall = image::DynamicImage::new_rgb8(300, 900);
        save_still(&tall, &out).unwrap();
        let saved = image::open(&out).unwrap();
        assert_eq!((saved.width(), saved.height()), (1920, 1080));
    }
}
