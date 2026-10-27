//! Game server: the live build plays on another server than EchoVRCE, a community
//! EchoRelay server or a Nakama server, until Back to EchoVRCE. The launcher asks the
//! player before it changes the server, and fills in the classic lobbies account for the
//! classic lobbies server when the name or password is left empty (this plugin never
//! sees that password).
use echovr_plugin::*;

#[derive(Default)]
struct S {
    info: Value,
    error: String,
    /// A server change the player is being asked about.
    asking: Option<u32>,
}

static STATE: State<S> = State::new();

struct GameServer;

impl Plugin for GameServer {
    fn init(info: &Value) -> Result<(), String> {
        STATE.with(|s| s.info = info.clone());
        Ok(())
    }

    fn render(ui: &mut Ui) {
        STATE.with(|s| render(ui, s));
    }

    fn on_event(e: Event) {
        STATE.with(|s| event(s, e));
        host::invalidate();
    }
}

export_plugin!(GameServer);

fn event(s: &mut S, e: Event) {
    match e {
        Event::LauncherChanged | Event::Shown => s.info = parse(&host::launcher_info()),
        Event::Clicked(id) => {
            s.error.clear();
            let set = match id.as_str() {
                "relay" => {
                    let address = match setting("relay_address") {
                        a if a.trim().is_empty() => text_at(&s.info, "relay.server"),
                        a => a.trim().to_string(),
                    };
                    game::set_server(&game::Server {
                        kind: "echorelay".into(),
                        name: String::new(),
                        address,
                        key: String::new(),
                        discord_id: String::new(),
                        display_name: setting("relay_name"),
                        password: setting("relay_password"),
                    })
                }
                "nakama" => {
                    if setting("address").trim().is_empty() {
                        s.error = "Fill in the Nakama server's address first.".into();
                        return;
                    }
                    game::set_server(&game::Server {
                        kind: "nakama".into(),
                        name: setting("name"),
                        address: setting("address"),
                        key: setting("key"),
                        discord_id: setting("discord_id"),
                        password: setting("password"),
                        display_name: String::new(),
                    })
                }
                "back" => {
                    if let Err(e) = game::reset_server() {
                        s.error = e;
                    }
                    s.info = parse(&host::launcher_info());
                    return;
                }
                _ => return,
            };
            match set {
                Ok(t) => s.asking = Some(t),
                Err(e) => s.error = e,
            }
        }
        Event::ServiceDone((t, r)) if s.asking == Some(t) => {
            s.asking = None;
            match r {
                Ok(_) => s.info = parse(&host::launcher_info()),
                Err(e) => s.error = e,
            }
        }
        _ => {}
    }
}

fn render(ui: &mut Ui, s: &S) {
    let server = |k: &str| text_at(&s.info, &format!("server.{k}"));
    let or = |a: String, b: String| if a.is_empty() { b } else { a };
    ui.card(0, "Where Echo VR plays", "", |c| {
        match server("kind").as_str() {
            _ if server("address").is_empty() => {
                c.text("On EchoVRCE.", Tone::Good);
            }
            "echorelay" => {
                c.value(
                    "On an EchoRelay server",
                    or(server("name"), server("address")),
                    false,
                );
                c.value("As", server("display_name"), false);
            }
            _ => {
                c.value(
                    "On a Nakama server",
                    or(server("name"), server("address")),
                    false,
                );
                c.value(
                    "Your Discord ID there",
                    or(server("discord_id"), "(none)".into()),
                    false,
                );
            }
        }
        if !s.error.is_empty() {
            c.warn(s.error.clone());
        }
        if !server("address").is_empty() {
            c.button(button("back", "Back to EchoVRCE"));
        }
    });
    ui.card(
        0,
        "EchoRelay server",
        "A community server. Its host gives you the address; your name and password are your account there. Empty: the classic lobbies and your account there.",
        |c| {
            c.form("EchoRelay");
            c.button(button("relay", "Play there").primary().enabled(s.asking.is_none()));
        },
    );
    ui.card(
        0,
        "Nakama server",
        "A server like EchoVRCE's (for example a mod pack's test server): what its host gives you. Its password is kept in plain text: not your Discord one.",
        |c| {
            c.form("Nakama");
            c.button(
                button("nakama", "Play there")
                    .enabled(!setting("address").trim().is_empty() && s.asking.is_none()),
            );
        },
    );
    ui.card(1, "How it works", "", |c| {
        c.muted("From the next start the live build signs in and finds matches on the server you picked, instead of EchoVRCE. Play's line says Server: and its name.");
        c.muted("On EchoRelay, Echo VR starts the way the event builds do, with the mods that are on: EchoVRCE's friends and parties are off there. Matches need a game server for the live build on that EchoRelay.");
        c.muted("Event builds keep the classic lobbies, and a mod pack with a server of its own (Settings, Launch options) keeps that one while it's on.");
        c.muted("Back to EchoVRCE puts everything back. Removing this plugin does too.");
    });
}
