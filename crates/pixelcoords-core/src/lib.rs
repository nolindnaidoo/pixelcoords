//! Platform-free core for pixelcoords: geometry, selections, session schema,
//! point verdicts, code emitters, template relocation, hotkey grammar,
//! config, bitmap font, and CPU rasterizer.
//!
//! The crate README is included below, which makes every example on the
//! crates.io page a compiled doctest — documentation that cannot rot
//! without failing CI.
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

pub mod config;
pub mod diff;
pub mod draw;
pub mod duration;
pub mod emit;
pub mod font;
pub mod geometry;
pub mod hotkeys;
pub mod locate;
pub mod matcher;
pub mod points;
pub mod report;
pub mod resolve;
pub mod selection;
pub mod session;
pub mod space;
pub mod strings;
pub mod verdict;
pub mod wait;
