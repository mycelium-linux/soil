// SPDX-License-Identifier: MIT OR Apache-2.0
//! Soil's Lisp: the S-expression reader, interpreter, typed core, WIT bindings,
//! and Wasm backends.

pub mod core;
pub mod eval;
pub mod lisp;
pub mod native_host;
pub mod records_host;
pub mod sexp;
pub mod time_host;
pub mod wasm;
pub mod witgen;

pub use core::{Expr, Function, Program, parse};
pub use eval::{FakeHost, Host, HostChain, Runtime, json_to_value as from_json, value_to_json as to_json};
pub use lisp::{Compiled as CompiledLisp, compile_source as compile_lisp};
pub use native_host::NativeHost;
pub use records_host::SoilRecordsHost;
pub use sexp::{Comment, Document, Form, Span, parse_document, print_document};
pub use time_host::SystemTimeHost;
pub use wasm::compile;
pub use witgen::{Generated, generate};
