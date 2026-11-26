//! Deploy Agent Skills from git repositories, local directories and
//! well-known discovery indexes, and agent definitions from git repositories
//! and local directories, declaratively, from a single `pollen.yaml`.

pub mod agent;
pub mod cli;
pub mod command;
pub mod config;
pub mod credentials;
pub mod deploy;
pub mod discovery;
pub mod hidden;
pub mod http;
pub mod schema;
pub mod shutdown;
pub mod skill;
pub mod source;
pub mod state;
pub mod version;
