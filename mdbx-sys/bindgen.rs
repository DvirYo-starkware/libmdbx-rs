use bindgen::callbacks::{IntKind, ParseCallbacks};
use std::{env, path::PathBuf};

#[derive(Debug)]
struct Callbacks;

impl ParseCallbacks for Callbacks {
    fn int_macro(&self, name: &str, _value: i64) -> Option<IntKind> {
        match name {
            "MDBX_SUCCESS"
            | "MDBX_KEYEXIST"
            | "MDBX_NOTFOUND"
            | "MDBX_PAGE_NOTFOUND"
            | "MDBX_CORRUPTED"
            | "MDBX_PANIC"
            | "MDBX_VERSION_MISMATCH"
            | "MDBX_INVALID"
            | "MDBX_MAP_FULL"
            | "MDBX_DBS_FULL"
            | "MDBX_READERS_FULL"
            | "MDBX_TLS_FULL"
            | "MDBX_TXN_FULL"
            | "MDBX_CURSOR_FULL"
            | "MDBX_PAGE_FULL"
            | "MDBX_MAP_RESIZED"
            | "MDBX_INCOMPATIBLE"
            | "MDBX_BAD_RSLOT"
            | "MDBX_BAD_TXN"
            | "MDBX_BAD_VALSIZE"
            | "MDBX_BAD_DBI"
            | "MDBX_LOG_DONTCHANGE"
            | "MDBX_DBG_DONTCHANGE"
            | "MDBX_RESULT_TRUE"
            | "MDBX_UNABLE_EXTEND_MAPSIZE"
            | "MDBX_PROBLEM"
            | "MDBX_LAST_LMDB_ERRCODE"
            | "MDBX_BUSY"
            | "MDBX_EMULTIVAL"
            | "MDBX_EBADSIGN"
            | "MDBX_WANNA_RECOVERY"
            | "MDBX_EKEYMISMATCH"
            | "MDBX_TOO_LARGE"
            | "MDBX_THREAD_MISMATCH"
            | "MDBX_TXN_OVERLAPPING"
            | "MDBX_LAST_ERRCODE" => Some(IntKind::Int),
            _ => Some(IntKind::UInt),
        }
    }
}

pub fn generate() {
    let mut mdbx = PathBuf::from(&env::var("CARGO_MANIFEST_DIR").unwrap());
    mdbx.push("libmdbx");

    let mut out_path = PathBuf::from(&env::var("CARGO_MANIFEST_DIR").unwrap());
    out_path.push("src");

    let bindings = bindgen::Builder::default()
        .header(mdbx.join("mdbx.h").to_string_lossy())
        .whitelist_var("^(MDBX|mdbx)_.*")
        .whitelist_type("^(MDBX|mdbx)_.*")
        .whitelist_function("^(MDBX|mdbx)_.*")
        .raw_line(
            "
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
",
        )
        .size_t_is_usize(true)
        .ctypes_prefix("::libc")
        .blacklist_item("mode_t")
        .blacklist_item("mdbx_filehandle_t")
        .parse_callbacks(Box::new(Callbacks))
        .layout_tests(false)
        .prepend_enum_name(false)
        .generate_comments(false)
        .disable_header_comment()
        .rustfmt_bindings(true)
        .generate()
        .expect("Unable to generate bindings");

    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}
