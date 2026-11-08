//! fusion-core: everything about the Fusion bridge that does not care which OS it is on.
//!
//! The two traits below are the ENTIRE OS boundary. `os-win` implements them for Windows
//! (ours, shipped); `os-mac` is the skeleton Kyle's macOS bridge implements against.
//! Everything else the bridge does to the desktop goes through Adom Bridge verbs
//! (`ab::call`), which are the same on both platforms - that is what makes this crate
//! portable for free. Never add a `windows` / `cocoa` dependency here; the CI check
//! `tools/check-portable.sh` fails the build if one appears.

pub mod install_check;
pub mod contract;
pub mod host;
pub mod alerts;
pub mod aps;
pub mod windows_facts;
pub mod dialogs;
pub mod readiness;
pub mod ab;
pub mod mech;

pub use host::{HostPaths, HostProcess};
