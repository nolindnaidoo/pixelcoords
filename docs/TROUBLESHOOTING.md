# Troubleshooting and FAQ

## Troubleshooting

**macOS captures only my wallpaper.** Screen Recording permission is
missing. Grant it under System Settings → Privacy & Security → Screen &
System Audio Recording — the grant attaches to your *terminal* app —
then quit and reopen the terminal. `pixelcoords doctor` tells you which
state you are in.

**Windows reports an odd scale factor.** `pixelcoords doctor` prints
`dpi awareness: per-monitor v2` on a healthy setup. If it says
otherwise, an application-compatibility override is forcing this
process into a different DPI mode, and reported scale factors will be
approximate. Clear the override under the executable's Properties →
Compatibility → Change high DPI settings.

**Linux says monitor geometry needs Xwayland.** The session has no
reachable X server. On a normal desktop this means `DISPLAY` was unset
or wrong — Xwayland itself starts on demand. Compositors built without
Xwayland cannot run pixelcoords; use an X11 session there.

**Linux build fails.** Install the build dependencies from the README
(Fedora: `libxcb-devel pipewire-devel clang-devel mesa-libEGL-devel
mesa-libgbm-devel`).

**`windows` or `--target` errors on Linux.** On Wayland this is
expected and permanent — the protocol does not expose window geometry.
Use `--pick` to mark a single window there, or an X11 session for full
targeting. On X11, enumeration needs an EWMH window manager; bare X
servers (Xvfb, kiosk setups) have none.

**Something crashed.** The crash message includes a report link —
please file it with the printed details and `pixelcoords doctor`
output.

## Behaviors worth knowing

- Nothing is saved automatically: `S` saves, and only `Esc` warns about
  unsaved work. Ctrl-C, `kill`, or a logout discard unsaved selections —
  which matters more than usual because the overlay covers the terminal
  you would type Ctrl-C into.
- **Wayland requires Xwayland.** Capture goes through the
  xdg-desktop-portal, but monitor geometry comes from RandR, which needs
  an X connection. Every mainstream desktop ships Xwayland and starts it
  on demand; nothing needs configuring.
- **Window bounds are the window you can see.** Toolkits draw an
  invisible shadow outside their windows — 26 px left/right, 23 top,
  29 bottom under GNOME 46 —
  and X11 reports that outer frame. `windows`, the `--target` outline,
  and every `window_px` coordinate are inset to the visible edge, so
  window-relative coordinates land where a click would.
- Crops are clipped to the screen; a shape dragged against an edge crops
  to what was actually visible.
- **`find` recognizes a display, it does not count them.** A session's
  monitors are matched against the ones attached now by name, size and
  scale — not by enumeration order, which shuffles across replugs,
  reboots, and dock/undock. Unplug a display and put it back in a
  different port and relocation still works. Two identical panels resolve
  deterministically: the one that held the session's index if it is still
  there, otherwise the lowest. What `find` will not do is relocate against
  a display whose resolution or scale changed — template matching survives
  a window moving, not the pixels underneath it being resampled — and it
  says so in those words rather than claiming the display is unplugged.

## FAQ

**Is it really offline?** Yes. There is no network code in the tree;
the only URL in the binary is the issue-reporting link in the crash
handler.

**Why freeze instead of a live overlay?** Precision and portability: on
a frozen image nothing moves between look and click, and a
frozen-snapshot overlay is the only model Wayland's security design
permits.

**Can I drive it from scripts?** Yes — that is half the product:
exit codes gate, output names are deterministic, `session.json` is
versioned, and `doctor`/`windows` speak `--json`. See
[CLI.md](CLI.md) and [OUTPUT.md](OUTPUT.md).

**When Windows and Linux?** Both are done — verified on real hardware,
not just CI. On Linux that means X11 with the full feature set and
Wayland with everything Wayland permits (screen coordinates plus
`--pick` window marking). That hand-verification covers the feature set
through 0.4.0, and macOS through 0.5.1. **No overlay run since 0.5.1 has
been driven by hand on any platform** — those releases pass CI everywhere
and have headless tests, which is not the same thing. 0.7.0's MCP server
is the exception, and only because it is headless: it adds no overlay
code and was driven end to end against a real session. Multi-monitor and
mixed-DPI are verified on macOS and remain test-only elsewhere, and
fractional scaling is unverified everywhere. The README's platform table
is kept honest, and this paragraph says the same thing it does.
