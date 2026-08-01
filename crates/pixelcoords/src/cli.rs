use std::path::PathBuf;

use clap::{Parser, Subcommand};

const EXAMPLES: &str = "\
Examples:
  pixelcoords                          freeze all monitors, mark, S saves
  pixelcoords --target \"Notepad\"       window-relative session
  pixelcoords resume                   pick a saved session, keep editing
  pixelcoords assert --session <dir> --point 812,440 --expect submit
  pixelcoords assert --session <dir> --stdin < points.txt
  pixelcoords resolve --session <dir> --label submit --units auto
  pixelcoords emit --session <dir> --format pyautogui
  pixelcoords diff --session <dir> --against baseline
  pixelcoords find --session <dir>
  pixelcoords doctor --json

Sessions save to Downloads/pixelcoords-captures/<timestamp> unless --out
says otherwise. Output reference:
https://github.com/nolindnaidoo/pixelcoords/blob/main/docs/OUTPUT.md";

/// Freeze your screen, mark regions, get pixel-exact coordinates and crops.
#[derive(Debug, Parser)]
#[command(name = "pixelcoords", version, about, after_help = EXAMPLES)]
pub struct Cli {
    /// Freeze only these monitors (default: all). Each is an index, the
    /// word `primary`, or part of a display's name — repeat the flag or
    /// separate with commas: --monitor primary,DELL
    #[arg(long, value_name = "QUERY", value_delimiter = ',')]
    pub monitor: Vec<String>,

    /// Attach to a window: matches its title (then app name), freezes that
    /// window's monitor, and adds window-relative coordinates to the output
    #[arg(long, value_name = "TITLE", conflicts_with = "monitor")]
    pub target: Option<String>,

    /// Linux: freeze one window chosen in the system picker (the Wayland
    /// answer to --target); every coordinate comes out window-relative
    #[arg(long, conflicts_with_all = ["target", "monitor"])]
    pub pick: bool,

    /// Output directory (default: Downloads/pixelcoords-captures/<timestamp>)
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,

    /// Friendly session name ("microsoft teams") shown by the resume
    /// picker; also settable in the overlay with N
    #[arg(long, value_name = "TEXT")]
    pub name: Option<String>,

    /// Config file path (default: the OS config directory)
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    #[arg(
        long,
        value_name = "SPEC",
        help = "Bind a key: KEY=ACTION[,EDGE][,WHEN]; repeatable.\nEDGE: press|release|repeat. WHEN: has_selection|cursor_in"
    )]
    pub bind: Vec<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check capture permissions, config, and the monitor setup; exits
    /// nonzero when anything is unhealthy, so scripts can gate on it
    Doctor {
        /// Config file to validate (default: the OS config directory)
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
        /// Machine-readable report on stdout instead of the human one
        #[arg(long)]
        json: bool,
    },
    /// List visible windows and their titles, for use with --target
    Windows {
        /// Machine-readable list on stdout instead of the human table
        #[arg(long)]
        json: bool,
    },
    /// Capture every monitor straight to PNG files, no overlay, no
    /// session — a plain scripted screenshot with the same DPI handling
    Shoot {
        /// Output directory (default: Downloads/pixelcoords-captures/<timestamp>)
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
        /// Capture only these monitors (default: all). Same grammar as the
        /// root --monitor: an index, `primary`, or part of a display's name
        #[arg(long, value_name = "QUERY", value_delimiter = ',')]
        monitor: Vec<String>,
    },
    /// Test one point, or a stream of them, against a saved session's
    /// regions: prints a JSON report; exits 0 on hit, 1 on miss, 2 on error
    Assert {
        /// Path to a session.json, or the directory containing one
        #[arg(long, value_name = "PATH")]
        session: PathBuf,
        /// The point to test, as X,Y physical pixels
        #[arg(
            long,
            value_name = "X,Y",
            allow_hyphen_values = true,
            required_unless_present = "stdin",
            conflicts_with = "stdin"
        )]
        point: Option<String>,
        /// Read points from stdin instead: one `X,Y` or `X,Y,label` per
        /// line, blank lines and `#` comments skipped. A per-line label
        /// overrides --expect for that line only
        #[arg(long)]
        stdin: bool,
        /// The label the point must land in for a hit (case-insensitive).
        /// A miss still reports every region it *did* land in
        #[arg(long, value_name = "TEXT")]
        expect: Option<String>,
        /// Accepted only to refuse it: this flag changed meaning, and
        /// silently doing the other thing would be worse than an error
        #[arg(long, value_name = "TEXT", hide = true)]
        label: Option<String>,
        /// Coordinate space the point is in
        #[arg(long, value_enum, default_value_t = SpaceArg::Global)]
        space: SpaceArg,
        /// Which monitor a `--space monitor` point is local to (optional
        /// on single-monitor sessions)
        #[arg(long, value_name = "INDEX")]
        monitor: Option<usize>,
    },
    /// Print ready-to-paste click snippets for an automation tool, one
    /// click per selection, in the tool's own coordinate convention
    Emit {
        /// Path to a session.json, or the directory containing one
        #[arg(long, value_name = "PATH")]
        session: PathBuf,
        /// The automation tool to generate for
        #[arg(long, value_enum)]
        format: FormatArg,
        /// Emit only selections with this label (case-insensitive)
        #[arg(long, value_name = "TEXT")]
        label: Option<String>,
    },
    /// Reopen a saved session for editing: the frozen screenshots come
    /// back as the canvas in resizable windows, every selection is
    /// editable again, and saves update the session in place
    Resume {
        /// A session directory or session.json path — or just the folder
        /// name under Downloads/pixelcoords-captures. Omit it to pick
        /// interactively from your saved sessions
        #[arg(long, value_name = "PATH|NAME")]
        session: Option<PathBuf>,
        /// Resume the most recent session, no questions asked
        #[arg(long, conflicts_with = "session")]
        last: bool,
        /// Save somewhere else instead of updating the session in place
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Give a saved session a friendly name for the resume picker
    Rename {
        /// A session directory, session.json path, or folder name under
        /// Downloads/pixelcoords-captures
        #[arg(long, value_name = "PATH|NAME")]
        session: PathBuf,
        /// The friendly name; an empty string clears it
        #[arg(long, value_name = "TEXT")]
        name: String,
    },
    /// Where to act for a session's labels, right now: the click point per
    /// selection in the space and units your API speaks. Exits 0 resolved,
    /// 1 not resolvable, 2 on error
    Resolve {
        /// Path to a session.json, or the directory containing one
        #[arg(long, value_name = "PATH")]
        session: PathBuf,
        /// Resolve only selections with this label (case-insensitive)
        #[arg(long, value_name = "TEXT")]
        label: Option<String>,
        /// Which origin the answer is measured from
        #[arg(long, value_enum, default_value_t = SpaceArg::Global)]
        space: SpaceArg,
        /// Which scale the answer is expressed in
        #[arg(long, value_enum, default_value_t = UnitsArg::Auto)]
        units: UnitsArg,
        /// Capture the screen first and correct for drift, so the answer
        /// describes where the region is now rather than where it was
        #[arg(long)]
        relocate: bool,
    },
    /// Compare a session's regions against the screen now, or against
    /// stored artifacts: prints a JSON report; exits 0 when every region
    /// is within tolerance, 1 otherwise, 2 on error
    Diff {
        /// Path to a session.json, or the directory containing one
        #[arg(long, value_name = "PATH")]
        session: PathBuf,
        /// Compare against another session's screenshots, or a single PNG
        /// standing in for a one-monitor session's capture, instead of
        /// capturing the screen
        #[arg(long, value_name = "DIR|IMAGE")]
        against: Option<PathBuf>,
        /// Compare only selections with this label (case-insensitive)
        #[arg(long, value_name = "TEXT")]
        label: Option<String>,
        /// Percent of a region's masked pixels allowed to differ before
        /// it fails. Default 0 — exact
        #[arg(long, value_name = "PCT", default_value_t = 0.0)]
        tolerance: f64,
    },
    /// Re-locate a session's regions in a fresh capture using their saved
    /// crops: prints a JSON report; exits 0 when every region is found
    /// unambiguously, 1 otherwise, 2 on error
    Find {
        /// Path to a session.json, or the directory containing one
        #[arg(long, value_name = "PATH")]
        session: PathBuf,
        /// Re-locate only selections with this label (case-insensitive)
        #[arg(long, value_name = "TEXT")]
        label: Option<String>,
    },
}

/// The automation tools `emit` can speak to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum FormatArg {
    /// Python: pyautogui.click(x, y) — logical points on macOS, physical
    /// pixels on Windows/X11
    Pyautogui,
    /// macOS shell: cliclick c:x,y — logical points
    Cliclick,
    /// X11 shell: xdotool mousemove x y click 1 — physical pixels
    Xdotool,
}

/// The scale a coordinate is expressed in. A separate question from
/// `--space`, which says where the origin is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum UnitsArg {
    /// Whatever this platform's input APIs expect: logical points on
    /// macOS, physical pixels on Windows and X11
    Auto,
    /// Device pixels — the session's own grid
    Physical,
    /// Points, device pixels divided by the monitor's DPI scale
    Logical,
}

impl From<UnitsArg> for pixelcoords_core::space::Units {
    fn from(arg: UnitsArg) -> Self {
        match arg {
            UnitsArg::Auto => Self::Auto,
            UnitsArg::Physical => Self::Physical,
            UnitsArg::Logical => Self::Logical,
        }
    }
}

/// Which of the session's coordinate spaces `assert --point` is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SpaceArg {
    /// Global desktop pixels (`global_px`)
    Global,
    /// Monitor-local pixels (`px`); pair with --monitor on multi-monitor
    /// sessions
    Monitor,
    /// Pixels relative to the --target window's top-left (`window_px`)
    Window,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_definition_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
