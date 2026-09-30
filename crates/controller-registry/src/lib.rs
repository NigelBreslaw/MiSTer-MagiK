// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
pub use mister_magik_core::input_info;
#[macro_export]
macro_rules! registry_logln {($($arg:tt)*)=>{{use std::io::Write;let _=writeln!(std::io::stderr().lock(),$($arg)*);}};}
mod db;
pub use db::*;
mod persistence;
#[cfg(any(test, feature = "io-probe"))]
pub mod probe;
pub use persistence::*;
