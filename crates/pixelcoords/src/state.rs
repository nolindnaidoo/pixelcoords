//! Cross-run UI state — a tiny app-owned file, kept separate from the
//! user's config.toml so automatic writes never touch what a human
//! maintains. Loading is tolerant where config loading is strict: a
//! corrupt state file costs a parked panel position, never a run.

use std::path::PathBuf;

use pixelcoords_core::geometry::Point;

fn state_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("pixelcoords").join("state.toml"))
}

/// Where the panel was parked last run, if anywhere.
pub fn load_panel() -> Option<(usize, Point)> {
    let text = std::fs::read_to_string(state_path()?).ok()?;
    parse_panel(&text)
}

/// Best-effort persist; failing to save UI state must never fail a run.
/// `None` (the panel was never moved) leaves the stored position alone.
pub fn save_panel(origin: Option<(usize, Point)>) {
    let (Some(path), Some((frame, p))) = (state_path(), origin) else {
        return;
    };
    let Some(dir) = path.parent() else { return };
    let text = format!("[panel]\nframe = {frame}\nx = {}\ny = {}\n", p.x, p.y);
    if let Err(e) = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, text)) {
        log::warn!("could not save UI state to {}: {e}", path.display());
    }
}

fn parse_panel(text: &str) -> Option<(usize, Point)> {
    let table: toml::Table = text.parse().ok()?;
    let panel = table.get("panel")?;
    let frame = usize::try_from(panel.get("frame")?.as_integer()?).ok()?;
    let x = i32::try_from(panel.get("x")?.as_integer()?).ok()?;
    let y = i32::try_from(panel.get("y")?.as_integer()?).ok()?;
    Some((frame, Point::new(x, y)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_state_round_trips_through_its_own_format() {
        let text = "[panel]\nframe = 1\nx = 320\ny = -4\n";
        assert_eq!(parse_panel(text), Some((1, Point::new(320, -4))));
    }

    #[test]
    fn corrupt_state_reads_as_absent_not_as_an_error() {
        assert_eq!(parse_panel("not toml ["), None);
        assert_eq!(parse_panel("[panel]\nframe = -2\nx = 1\ny = 1\n"), None);
        assert_eq!(parse_panel("[panel]\nx = 1\n"), None);
        assert_eq!(parse_panel(""), None);
    }
}
