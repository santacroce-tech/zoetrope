//! Zoetrope core: the single source of truth for scene structure, geometry,
//! edits/undo, serialization and rendering. Contains no platform code, so it
//! runs headless in tests and compiles unchanged to WASM for both the editor
//! preview and the exported player.

pub mod asset;
pub mod color;
pub mod demo;
pub mod edit;
pub mod error;
pub mod format;
pub mod geom;
pub mod history;
pub mod interact;
pub mod math;
pub mod model;
pub mod ops;
pub mod outline;
pub mod paint;
pub mod player;
pub mod query;
pub mod render;
pub mod text;
pub mod timeline;
pub mod vector;

pub use color::{Color, ColorTransform};
pub use error::{Error, Result};
pub use history::Document;
pub use math::{Matrix, Point};
pub use model::*;
