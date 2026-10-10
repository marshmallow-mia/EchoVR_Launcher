//! `EchoVR_Launcher --tray`: an icon in the system tray, and no window. While no launcher
//! window is open it looks for updates every 15 minutes (`core::updates`) and announces
//! the new ones on the desktop; its menu opens the launcher, looks now, or quits. While a
//! window is open, the window looks itself (and this one waits).
//!
//! Linux: a StatusNotifierItem (KDE; GNOME with the AppIndicator extension). Windows: a
//! notification-area icon. Not on macOS.

// No tray on macOS: much of this is for Windows and Linux only.
#![cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]

use std::time::{Duration, Instant};

use crate::core::launcher::store::LauncherState;
use crate::core::{tray, updates};

const TITLE: &str = "Echo VR Launcher";

/// The tray's own run; the process' exit code.
pub fn main() -> i32 {
    if !cfg!(any(windows, target_os = "linux")) {
        return 0;
    }
    if tray::running() {
        tracing::info!("tray: another one runs");
        return 0;
    }
    tracing::info!("tray: running");
    run()
}

/// What the icon's tooltip says.
fn status_text(found: &[updates::Finding]) -> String {
    match found.len() {
        0 => "Everything is up to date".into(),
        1 => found[0].text(),
        n => format!("{n} updates available"),
    }
}

/// Looks for updates whenever due (the window isn't open, or was asked to), announcing new
/// ones; `status` gets what was found.
fn checker(status: impl Fn(String) + Send + 'static) {
    std::thread::spawn(move || {
        let mut last: Option<Instant> = None;
        loop {
            let asked = tray::take_check();
            let due = last.is_none_or(|t| t.elapsed() >= updates::INTERVAL);
            if (asked || due) && !tray::window_running() {
                let state = LauncherState::load();
                if !state.tray {
                    tracing::info!("tray: turned off in Settings");
                    std::process::exit(0);
                }
                let check = updates::check(&state);
                let fresh = updates::not_yet_announced(&check.findings);
                if state.desktop_notifications {
                    updates::notify_desktop(&fresh);
                }
                status(status_text(&check.findings));
                last = Some(Instant::now());
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

/// The launcher's icon as RGBA (width, height, pixels).
fn icon_rgba() -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(crate::core::desktop_notify::ICON_PNG)
        .ok()?
        .resize(64, 64, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    Some((img.width(), img.height(), img.into_raw()))
}

// ---- Linux ----

#[cfg(target_os = "linux")]
struct Sni {
    status: String,
    icon: Vec<ksni::Icon>,
}

#[cfg(target_os = "linux")]
impl ksni::Tray for Sni {
    fn id(&self) -> String {
        "echovr-launcher".into()
    }

    fn title(&self) -> String {
        TITLE.into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: TITLE.into(),
            description: self.status.clone(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        tray::open_window();
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Open Echo VR Launcher".into(),
                activate: Box::new(|_| tray::open_window()),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Check for updates now".into(),
                activate: Box::new(|_| tray::ask_check()),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit (no more update checks)".into(),
                activate: Box::new(|_| std::process::exit(0)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

#[cfg(target_os = "linux")]
fn run() -> i32 {
    use ksni::blocking::TrayMethods;
    let icon = icon_rgba()
        .map(|(w, h, rgba)| {
            // ARGB32, network byte order.
            let (pixels, _) = rgba.as_chunks::<4>();
            let data = pixels
                .iter()
                .flat_map(|p| [p[3], p[0], p[1], p[2]])
                .collect();
            vec![ksni::Icon {
                width: w as i32,
                height: h as i32,
                data,
            }]
        })
        .unwrap_or_default();
    let sni = Sni {
        status: "Looking for updates…".into(),
        icon,
    };
    let handle = match sni.spawn() {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("tray: no tray here ({e}); looking for updates without an icon");
            let (tx, rx) = std::sync::mpsc::channel::<String>();
            checker(move |s| {
                let _ = tx.send(s);
            });
            while rx.recv().is_ok() {}
            return 0;
        }
    };
    let h = handle.clone();
    checker(move |s| {
        h.update(|t| t.status = s);
    });
    while !handle.is_closed() {
        std::thread::sleep(Duration::from_secs(1));
    }
    0
}

// ---- Windows ----

#[cfg(windows)]
fn run() -> i32 {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };

    let menu = Menu::new();
    let open = MenuItem::new("Open Echo VR Launcher", true, None);
    let check = MenuItem::new("Check for updates now", true, None);
    let quit = MenuItem::new("Quit (no more update checks)", true, None);
    let _ = menu.append_items(&[&open, &check, &PredefinedMenuItem::separator(), &quit]);
    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip(TITLE);
    if let Some(icon) =
        icon_rgba().and_then(|(w, h, rgba)| tray_icon::Icon::from_rgba(rgba, w, h).ok())
    {
        builder = builder.with_icon(icon);
    }
    let icon = match builder.build() {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("tray: no icon ({e})");
            return 1;
        }
    };
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    checker(move |s| {
        let _ = tx.send(s);
    });
    loop {
        // The icon's messages come through this thread's message queue.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        while let Ok(e) = MenuEvent::receiver().try_recv() {
            if e.id == *open.id() {
                tray::open_window();
            } else if e.id == *check.id() {
                tray::ask_check();
            } else if e.id == *quit.id() {
                return 0;
            }
        }
        while let Ok(e) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                tray::open_window();
            }
        }
        while let Ok(s) = rx.try_recv() {
            let _ = icon.set_tooltip(Some(format!("{TITLE}: {s}")));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn run() -> i32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_was_found() {
        assert_eq!(status_text(&[]), "Everything is up to date");
        let f = updates::Finding::Launcher {
            version: "0.11.0".into(),
            url: String::new(),
        };
        assert_eq!(
            status_text(std::slice::from_ref(&f)),
            "Echo VR Launcher 0.11.0 is out"
        );
        assert_eq!(status_text(&[f.clone(), f]), "2 updates available");
        assert!(icon_rgba().is_some_and(|(w, h, px)| px.len() == (w * h * 4) as usize));
    }
}
