#![deny(warnings)]
#![allow(non_camel_case_types, non_upper_case_globals)]
#![allow(clippy::all)]
#![doc(html_root_url = "https://docs.rs/mdbx-sys/0.9.3")]

extern crate libc;

#[cfg(unix)]
#[allow(non_camel_case_types)]
pub type mode_t = ::libc::mode_t;
#[cfg(windows)]
#[allow(non_camel_case_types)]
pub type mode_t = ::libc::c_int;

#[cfg(unix)]
#[allow(non_camel_case_types)]
pub type mdbx_filehandle_t = ::libc::c_int;
#[cfg(windows)]
#[allow(non_camel_case_types)]
pub type mdbx_filehandle_t = *mut ::libc::c_void;

include!("bindings.rs");
