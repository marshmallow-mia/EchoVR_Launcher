//! An error that already knows how the UI should present it.

use std::fmt;

/// Which help link the error dialog shows under the message (the Java `hyperlink` int).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HelpLink {
    #[default]
    None,
    /// "How to enable Developer Mode on your Quest"
    DeveloperMode,
    /// "How to allow USB debugging on your Quest"
    UsbDebugging,
}

#[derive(Debug, Clone)]
pub struct UiError {
    pub title: String,
    pub message: String,
    pub link: HelpLink,
}

impl UiError {
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        UiError {
            title: title.into(),
            message: message.into(),
            link: HelpLink::None,
        }
    }

    pub fn with_link(mut self, link: HelpLink) -> Self {
        self.link = link;
        self
    }

    /// Presents any error: a `UiError` as itself, anything else under `fallback_title`.
    pub fn from_anyhow(e: &anyhow::Error, fallback_title: &str) -> UiError {
        match e.downcast_ref::<UiError>() {
            Some(u) => u.clone(),
            None => UiError::new(fallback_title, format!("{e:#}")),
        }
    }
}

impl fmt::Display for UiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.title, self.message)
    }
}

impl std::error::Error for UiError {}

pub const QUEST_UNAUTHORIZED_TITLE: &str = "Allow your PC on the Quest";
pub const QUEST_UNAUTHORIZED: &str = "Your Quest is connected, but it hasn't allowed this PC yet.\nPut on your headset and tap Allow when the USB debugging prompt appears\n(replug the cable if you don't see it).";
pub const QUEST_NOT_FOUND_TITLE: &str = "No Quest detected";
pub const QUEST_NOT_FOUND: &str =
    "Couldn't find your Quest. Connect it by USB and make sure\nDeveloper Mode is enabled.";

pub fn quest_unauthorized() -> UiError {
    UiError::new(QUEST_UNAUTHORIZED_TITLE, QUEST_UNAUTHORIZED).with_link(HelpLink::UsbDebugging)
}

pub fn quest_not_found() -> UiError {
    UiError::new(QUEST_NOT_FOUND_TITLE, QUEST_NOT_FOUND).with_link(HelpLink::DeveloperMode)
}
