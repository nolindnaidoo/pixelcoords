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
/// Internal: a CPU rasterizer tied to softbuffer's pixel format. Public only because the binary is a
/// separate crate; not part of this crate's API.
#[doc(hidden)]
pub mod draw;
pub mod duration;
pub mod emit;
/// Internal: the embedded overlay font. Public only because the binary is a
/// separate crate; not part of this crate's API.
#[doc(hidden)]
pub mod font;
pub mod geometry;
/// Internal: the overlay's key-binding grammar. Public only because the binary is a
/// separate crate; not part of this crate's API.
#[doc(hidden)]
pub mod hotkeys;
pub mod locate;
/// Internal: `--target` window matching. Public only because the binary is a
/// separate crate; not part of this crate's API.
#[doc(hidden)]
pub mod matcher;
pub mod points;
pub mod report;
pub mod resolve;
/// Internal: overlay editing state and undo stacks. Public only because the binary is a
/// separate crate; not part of this crate's API.
#[doc(hidden)]
pub mod selection;
pub mod session;
/// Internal: overlay edge snapping. Public only because the binary is a
/// separate crate; not part of this crate's API.
#[doc(hidden)]
pub mod snap;
pub mod space;
/// Internal: the overlay's user-facing string table. Public only because the binary is a
/// separate crate; not part of this crate's API.
#[doc(hidden)]
pub mod strings;
pub mod verdict;
pub mod wait;
