//! 1c-connect: the developer's side of the environment. A tunnel into the
//! container with an embedded SSH client, and a bridge that starts mcp-1c on
//! the developer's base behind each forwarded connection. Nothing is
//! installed on this machine; the program lives on a flash drive.

pub mod allow;
pub mod analyzer;
pub mod bridge;
pub mod cfg;
pub mod editors;
pub mod flow;
pub mod gate;
#[cfg(feature = "gui")]
pub mod gui;
pub mod holder;
pub mod job;
pub mod jsonrpc;
pub mod licence;
pub mod listen;
pub mod local;
pub mod onec_error;
pub mod ops_host;
pub mod platform;
pub mod project;
pub mod publication;
pub mod secrets;
pub mod session;
pub mod single_instance;
pub mod skills;
pub mod tunnel;
pub mod verify;
