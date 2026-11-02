//! session-board: every Claude Code session on this machine in one terminal app, replacing
//! Agent View. This crate holds the facts (Claude Code's files → sessions and states), the rules
//! for acting on sessions, the settings, and the terminal app (`tui`); `main.rs` is the command
//! line.

pub mod actions;
pub mod cache;
pub mod claude;
pub mod config;
pub mod facts;
pub mod git;
pub mod mask;
pub mod proc;
pub mod report;
pub mod shot;
pub mod store;
pub mod tui;
