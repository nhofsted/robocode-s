// SPDX-License-Identifier: EPL-2.0

//! Client of the rumble backend (docs/rumble-backend.md §13): the HTTP API, the GitHub
//! device-flow login, the token file and the jar cache. Blocking, no async runtime.

pub mod backoff;
pub mod http;
pub mod jars;
pub mod login;
pub mod tokenfile;
