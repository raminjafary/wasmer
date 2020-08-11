//! Unofficial API for WASI integrating with the standard Wasm C API.
//!
//! This API will be superseded by a standard WASI API when/if such a standard is created.

mod capture_files;

use super::{wasm_extern_t, wasm_memory_t, wasm_module_t, wasm_store_t};
// required due to really weird Rust resolution rules for macros
// https://github.com/rust-lang/rust/issues/57966
use crate::c_try;
use std::convert::TryFrom;
use std::ffi::CStr;
use std::io::Read;
use std::mem;
use std::os::raw::c_char;
use std::ptr::NonNull;
use std::slice;
use wasmer::{Extern, NamedResolver, Store};
use wasmer_wasi::{
    generate_import_object_from_env, get_wasi_version, WasiEnv, WasiFile, WasiState,
    WasiStateBuilder, WasiVersion,
};

#[no_mangle]
pub unsafe extern "C" fn wasi_state_builder_new(
    program_name: *const c_char,
) -> Option<Box<WasiStateBuilder>> {
    debug_assert!(!program_name.is_null());

    let name_c_str = CStr::from_ptr(program_name);
    let prog_name = c_try!(name_c_str.to_str());

    Some(Box::new(WasiState::new(prog_name)))
}

#[no_mangle]
pub unsafe extern "C" fn wasi_state_builder_env(
    state_builder: &mut WasiStateBuilder,
    key: *const c_char,
    value: *const c_char,
) {
    debug_assert!(!key.is_null());
    debug_assert!(!value.is_null());

    let key_cstr = CStr::from_ptr(key);
    let key_bytes = key_cstr.to_bytes();
    let value_cstr = CStr::from_ptr(value);
    let value_bytes = value_cstr.to_bytes();

    state_builder.env(key_bytes, value_bytes);
}

#[no_mangle]
pub unsafe extern "C" fn wasi_state_builder_arg(
    state_builder: &mut WasiStateBuilder,
    arg: *const c_char,
) {
    debug_assert!(!arg.is_null());

    let arg_cstr = CStr::from_ptr(arg);
    let arg_bytes = arg_cstr.to_bytes();

    state_builder.arg(arg_bytes);
}

// NOTE: don't modify this type without updating all users of it. We rely on
// this struct being `repr(transparent)` with `Box<dyn WasiFile>` in the API.
#[repr(transparent)]
pub struct wasi_file_handle_t {
    inner: Box<dyn WasiFile>,
}

#[no_mangle]
pub unsafe extern "C" fn wasi_output_capturing_file_new() -> Box<wasi_file_handle_t> {
    Box::new(wasi_file_handle_t {
        inner: Box::new(capture_files::OutputCapturer::new()),
    })
}

#[no_mangle]
pub unsafe extern "C" fn wasi_file_handle_delete(_file_handle: Option<Box<wasi_file_handle_t>>) {}

/// returns the amount written to the buffer
#[no_mangle]
pub unsafe extern "C" fn wasi_output_capturing_file_read(
    wasi_file: &mut wasi_file_handle_t,
    buffer: *mut c_char,
    buffer_len: usize,
    start_offset: usize,
) -> isize {
    let inner_buffer = slice::from_raw_parts_mut(buffer as *mut _, buffer_len as usize);
    if let Some(oc) = wasi_file
        .inner
        .downcast_ref::<capture_files::OutputCapturer>()
    {
        (&oc.buffer[start_offset..])
            .read(inner_buffer)
            .unwrap_or_default() as isize
    } else {
        -1
    }
}

/// Override the Stdout that the WASI program will see.
///
/// This function takes ownership of the `wasi_file_handle_t` passed in.
///
/// The `wasi_file_handle_t` cannot be used after calling this function.
#[no_mangle]
pub unsafe extern "C" fn wasi_state_builder_set_stdout(
    state_builder: &mut WasiStateBuilder,
    stdout: Box<wasi_file_handle_t>,
) {
    state_builder.stdout(stdout.inner);
}

/// Override the Stderr that the WASI program will see.
///
/// This function takes ownership of the `wasi_file_handle_t' passed in.
///
/// The `wasi_file_handle_t` cannot be used after calling this function.
#[no_mangle]
pub unsafe extern "C" fn wasi_state_builder_set_stderr(
    state_builder: &mut WasiStateBuilder,
    stderr: Box<wasi_file_handle_t>,
) {
    state_builder.stderr(stderr.inner);
}

// NOTE: don't modify this type without updating all users of it. We rely on
// this struct being `repr(transparent)` with `WasiState` in the API.
#[allow(non_camel_case_types)]
#[repr(transparent)]
pub struct wasi_state_t {
    inner: WasiState,
}

/// This function takes ownership of the `wasi_state_builder`.
#[no_mangle]
pub extern "C" fn wasi_state_builder_build(
    mut state_builder: Box<WasiStateBuilder>,
) -> Option<Box<wasi_state_t>> {
    let inner = c_try!(state_builder.build());
    Some(Box::new(wasi_state_t { inner }))
}

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct wasi_env_t {
    inner: WasiEnv,
}

/// Takes ownership over the `wasi_state_t`.
#[no_mangle]
pub extern "C" fn wasi_env_new(state: Box<wasi_state_t>) -> Box<wasi_env_t> {
    Box::new(wasi_env_t {
        inner: WasiEnv::new(state.inner),
    })
}

#[no_mangle]
pub extern "C" fn wasi_env_delete(_state: Option<Box<wasi_env_t>>) {}

#[no_mangle]
pub extern "C" fn wasi_env_set_memory(env: &mut wasi_env_t, memory: &wasm_memory_t) {
    env.inner.set_memory(memory.inner.clone());
}

#[no_mangle]
pub extern "C" fn wasi_env_borrow_state(env: &wasi_env_t) -> &wasi_state_t {
    let state: &WasiState = &*env.inner.state();
    // This is correct because `wasi_state_t` is `repr(transparent)` to `WasiState`
    unsafe { mem::transmute(state) }
}

/// returns a non-owning reference to stdout
#[no_mangle]
pub extern "C" fn wasi_state_get_stdout(
    state: &wasi_state_t,
) -> Option<&Option<wasi_file_handle_t>> {
    let inner: &Option<Box<dyn WasiFile>> = c_try!(state.inner.fs.stdout());
    // This is correct because `wasi_file_handle_t` is `repr(transparent)` to `Box<dyn WasiFile>`
    let temp = unsafe { mem::transmute::<_, &'static Option<wasi_file_handle_t>>(inner) };
    Some(temp)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
#[allow(non_camel_case_types)]
pub enum wasi_version_t {
    Latest = 0,
    Snapshot0 = 1,
    Snapshot1 = 2,
    InvalidVersion = u32::max_value(),
}

impl From<WasiVersion> for wasi_version_t {
    fn from(other: WasiVersion) -> Self {
        match other {
            WasiVersion::Snapshot0 => wasi_version_t::Snapshot0,
            WasiVersion::Snapshot1 => wasi_version_t::Snapshot1,
            WasiVersion::Latest => wasi_version_t::Latest,
        }
    }
}

impl TryFrom<wasi_version_t> for WasiVersion {
    type Error = &'static str;

    fn try_from(other: wasi_version_t) -> Result<Self, Self::Error> {
        Ok(match other {
            wasi_version_t::Snapshot0 => WasiVersion::Snapshot0,
            wasi_version_t::Snapshot1 => WasiVersion::Snapshot1,
            wasi_version_t::Latest => WasiVersion::Latest,
            wasi_version_t::InvalidVersion => return Err("Invalid WASI version cannot be used"),
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn wasi_get_wasi_version(module: &wasm_module_t) -> wasi_version_t {
    get_wasi_version(&module.inner, false)
        .map(Into::into)
        .unwrap_or(wasi_version_t::InvalidVersion)
}

/// Takes ownership of `wasi_env_t`.
#[no_mangle]
pub unsafe extern "C" fn wasi_get_imports(
    store: Option<NonNull<wasm_store_t>>,
    module: &wasm_module_t,
    wasi_env: &wasi_env_t,
    version: wasi_version_t,
) -> Option<Box<[Box<wasm_extern_t>]>> {
    let store_ptr = store?.cast::<Store>();
    let store = store_ptr.as_ref();

    // TODO:
    //let version = c_try!(WasiVersion::try_from(version));
    let version = WasiVersion::try_from(version).ok()?;

    let import_object = generate_import_object_from_env(store, wasi_env.inner.clone(), version);

    // TODO: this is very inefficient due to all the allocation required
    let mut extern_vec = vec![];
    for it in module.inner.imports() {
        // TODO: return an error message here if it's not found
        let export = import_object.resolve_by_name(it.module(), it.name())?;
        let inner = Extern::from_export(store, export);
        extern_vec.push(Box::new(wasm_extern_t {
            instance: None,
            inner,
        }));
    }

    Some(extern_vec.into_boxed_slice())
}
