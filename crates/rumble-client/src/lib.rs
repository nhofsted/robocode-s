// SPDX-License-Identifier: EPL-2.0

//! Client of the rumble backend (docs/rumble-backend.md §13): the HTTP API, the GitHub
//! device-flow login, the token file, the jar cache, and running battles with their hashes
//! and tracks. Blocking, no async runtime.

pub mod backoff;
pub mod battle;
pub mod hash;
pub mod http;
pub mod import;
pub mod jars;
pub mod login;
pub mod tokenfile;
pub mod track;
