# S0-7: Split Play control in the current launcher

This adapts the S0-7 Play split design to the current EchoVR_Launcher UI. It places the
Play, Check for updates and platform controls in the logo band, gives the PC Play control a
separate version arrow, removes the standalone version picker, and keeps the selected
version name and progress visible. Install keeps its existing hero controls.

## Layout and behavior

Geometry is in the 1920×1080 design coordinate system before the launcher zoom. The
approved Play design uses a compact row at y=88..155. On PC the main action occupies
x=139..300, the arrow x=300..445, and Check for updates starts at x=445. The arrow uses a
clipped polygon hit region; inert slanted corners and seam strokes activate no control.
The menu is anchored below the arrow with the existing thin positioning anchor, and only
the active arrow polygon is excluded from outside-click dismissal. The blue update region
and inert corners remain outside that exclusion and dismiss an open menu.

The main action routes the selected installed, missing or catalogue version through the
existing launch, preflight and install paths. The visible name and state/progress occupy the
info band; optional details follow. With no version choices, the arrow is absent and the
full green control remains the main action. With one or more choices, the arrow exposes the
installed and catalogue rows, checked selection, and Install another version action.

The current destination keeps `QUEST` disabled and presents Quest as “coming soon.” The
ported row geometry and progress clipping support the Quest layout, and the Quest snapshot
fixtures exercise progress rendering, but the interactive PC/Quest switch does not enable
Quest or alter the saved PC selection. Clicking the coming-soon control closes an open
version menu and shows the existing coming-soon signal.

Determinate and indeterminate Play progress are clipped to the active green main bounds.
The PC split main remains capped at x<300 design pixels; Quest/no-arrow layouts use the full
green main. Popup placement, geometry and pointer ownership are tested at 960×540,
1280×720 and 1680×720.

## Acceptance coverage

- **BAC-0001:** compact Play geometry, selected-version information, progress clipping,
  Install layout preservation and retired logo hotspot.
- **BAC-0002:** separate accessible main/arrow controls, physical pointer and keyboard
  activation, inert-corner dismissal, opener-only exclusion, later blue Update dismissal,
  popup scrolling, and zero/one/many choice behavior.
- **BAC-0003:** installed and missing targets route through the selected version; installed
  B is chosen from the menu and its path reaches preflight. Missing targets send only a
  real catalogue ID to Install.
- **BAC-0004:** disabled reasons and rejected pointer/keyboard attempts during launch-start,
  owned/external running, selected job and unrelated job; selected ID retention; platform
  coming-soon transition; no stale menu.

The executable acceptance evidence and checks are recorded in
[`s0-7-port-evidence.md`](s0-7-port-evidence.md).
