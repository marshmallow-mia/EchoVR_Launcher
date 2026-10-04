//! Everything that is not UI: downloads, adb, updates, OAuth, OS integration.

pub mod adb;
pub mod cache;
pub mod download;
pub mod echovrce;
pub mod echoxr;
pub mod echoxr_hands;
pub mod elevation;
pub mod error;
pub mod http;
pub mod launcher;
pub mod links;
pub mod linux;
pub mod log;
pub mod logs;
pub mod manifest;
pub mod oauth;
pub mod paths;
pub mod pc_update;
pub mod platform;
pub mod process;
pub mod quest_install;
pub mod quest_update;
pub mod remote_zip;
pub mod revive;
pub mod zip;

/// The Echo VR Lounge, the community's main Discord.
pub const LOUNGE_INVITE: &str = "https://discord.com/invite/echo-vr-lounge";
