//! Controllers: EchoXR's controller settings (`EchoXR.txt` beside the game, in every
//! installed version) as the launcher's forms, a card per section, and presets. The
//! launcher keeps the values: this plugin only lays the page out.
use echovr_plugin::*;

struct Controllers;

impl Plugin for Controllers {
    fn init(_info: &Value) -> Result<(), String> {
        Ok(())
    }

    fn render(ui: &mut Ui) {
        for (section, title) in [
            ("hands", "Hands"),
            ("sticks", "Sticks and triggers"),
            ("vibration", "Vibration"),
            ("buttons", "Buttons"),
            ("diagnostics", "Diagnostics"),
        ] {
            ui.card(0, title, "", |c| {
                c.form(section);
            });
        }
        ui.card(1, "Presets", "", |c| {
            c.presets();
        });
        ui.card(1, "How it works", "", |c| {
            c.muted("These settings apply when Echo VR plays through EchoXR: SteamVR, Virtual Desktop, WiVRn or Monado. Changes apply right away, also while you play.");
            c.muted("Export… saves your settings as a file; Import… adds one someone gave you.");
            c.muted("Needs EchoXR 0.6 or newer.");
        });
    }

    fn on_event(_e: Event) {}
}

export_plugin!(Controllers);
