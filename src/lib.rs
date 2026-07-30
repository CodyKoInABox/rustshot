//! Core types and platform-independent behavior for Rustshot.

pub mod annotation;
pub mod config;
pub mod encode;
pub mod frame;

#[cfg(target_os = "windows")]
pub mod platform;
