//! Platform-free core for pixelcoords: geometry, selections, session schema,
//! point verdicts, code emitters, template relocation, hotkey grammar,
//! config, bitmap font, and CPU rasterizer.
#![forbid(unsafe_code)]

pub mod config;
pub mod draw;
pub mod emit;
pub mod font;
pub mod geometry;
pub mod hotkeys;
pub mod locate;
pub mod matcher;
pub mod selection;
pub mod session;
pub mod strings;
pub mod verdict;
