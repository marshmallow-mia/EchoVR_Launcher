# S0-7 port evidence

This change ports the split Play control to `marshmallow-mia/EchoVR_Launcher`, whose base
already contains the nEVR ownership of `bin/win10/BugSplat64.dll` in `src/core/launcher`
and `docs/launcher/mods.md`. The old ADR decision is therefore already implemented and
documented here; this port does not add a duplicate decision document.

## Checks

Run `just check` to execute formatting, Clippy with warnings denied, the full test suite,
and a release build in sequence. Cargo uses two jobs by default; set `CARGO_BUILD_JOBS=1`
to lower the cap. Focused Play tests are `just test ui::launcher::play::split_info_tests`.

## Progress render fixtures

The `play_quest_installing_42` fixture uses a named 42% fraction; `play_quest_installing`
uses a named 100% fraction. Render either fixture at each size with the headless snapshot
test (GPU required):

```sh
ECHOVR_SNAPSHOTS=docs/design/s0-7-renders/quest-installing-42-960 \
ECHOVR_SNAPSHOTS_ONLY='=launcher_play_quest_installing_42' ECHOVR_SNAPSHOTS_SIZE=960x540 \
cargo test -j 2 headless_snapshots -- --ignored --nocapture
```

Change the output directory and size for each row below. The committed files let reviewers
compare progress extent and the Quest info band at the minimum, default and wide viewport.

| Fixture | Size | Artifact |
| --- | --- | --- |
| 42% | 960×540 | `s0-7-renders/quest-installing-42-960/launcher_play_quest_installing_42.png` |
| 42% | 1280×720 | `s0-7-renders/quest-installing-42-1280/launcher_play_quest_installing_42.png` |
| 42% | 1680×720 | `s0-7-renders/quest-installing-42-1680/launcher_play_quest_installing_42.png` |
| 100% | 960×540 | `s0-7-renders/quest-installing-100-960/launcher_play_quest_installing.png` |
| 100% | 1280×720 | `s0-7-renders/quest-installing-100-1280/launcher_play_quest_installing.png` |
| 100% | 1680×720 | `s0-7-renders/quest-installing-100-1680/launcher_play_quest_installing.png` |

100% is the full-fill endpoint check; 42% confirms intermediate clipping rather than a
binary empty/full render. Automated geometry assertions independently check that PC fill
ends before x=300 design pixels and that Quest/no-arrow fill uses the active full main
bounds in both determinate and indeterminate states.

## Interactive acceptance tests

`ui::launcher::play::split_info_tests` covers the selected installed/catalogue names,
empty and single-choice menus, persistence failure/success, physical arrow opener and blue
Update dismissal, inert upper-right arrow-corner dismissal, polygon ownership at all three
sizes, installed-B menu selection and preflight path, missing-target Install routing, busy
disabled reasons plus pointer/keyboard rejection, and the current Quest coming-soon
transition with selection retained. `ui::launcher::hero::play_split_tests` covers progress
bounds and viewport geometry.
