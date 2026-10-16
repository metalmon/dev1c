//! 1c-connect: the developer's side of the environment. A tunnel into the
//! container with an embedded SSH client, and a bridge that starts mcp-1c on
//! the developer's base behind each forwarded connection. Nothing is
//! installed on this machine; the program lives on a flash drive.

pub mod bridge;
pub mod cfg;
#[cfg(feature = "gui")]
pub mod gui;
pub mod jsonrpc;
pub mod ops_host;
pub mod project;
pub mod session;
pub mod tunnel;
pub mod verify;
