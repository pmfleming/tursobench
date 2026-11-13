//! TursoBench's assistant runs on the agent CLI the user already installed and
//! signed in to. This crate never reads, stores or forwards credentials: it
//! starts the vendor's own unmodified binary and serves it database tools.
pub mod agent;
pub mod claude;
pub mod mcp;
pub mod relay;
