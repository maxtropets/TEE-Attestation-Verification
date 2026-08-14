// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! C ABI entry points. Nodes are exchanged as a flat indexed array; see
//! `include/tav/cbor.h` for the C declarations.
//!
//! Every entry point runs its body under [`std::panic::catch_unwind`], so a
//! panic is reported as a status code rather than unwinding into a C frame,
//! which would abort the host process.

use crate::{
    parse_det, parse_nondet, serialize_det, serialize_nondet, CborNode, STATUS_DECODE_FAILED,
    STATUS_ENCODE_FAILED, STATUS_OK,
};

/// Copy `msg` into a caller-owned buffer, to be released with [`tav_cbor_buffer_free`].
unsafe fn set_error(msg: &str, err_ptr: *mut *mut u8, err_len: *mut usize) {
    if err_ptr.is_null() || err_len.is_null() {
        return;
    }
    let mut bytes = msg.as_bytes().to_vec().into_boxed_slice();
    let len = bytes.len();
    let ptr = bytes.as_mut_ptr();
    std::mem::forget(bytes);
    unsafe {
        *err_ptr = ptr;
        *err_len = len;
    }
}

unsafe fn input_slice<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

unsafe fn finish_parse(
    result: Result<Vec<CborNode>, String>,
    out_ptr: *mut *mut CborNode,
    out_len: *mut usize,
    err_ptr: *mut *mut u8,
    err_len: *mut usize,
) -> i32 {
    match result {
        Ok(nodes) => {
            let mut nodes = nodes.into_boxed_slice();
            let len = nodes.len();
            let ptr = nodes.as_mut_ptr();
            std::mem::forget(nodes);
            unsafe {
                *out_ptr = ptr;
                *out_len = len;
            }
            STATUS_OK
        }
        Err(e) => {
            unsafe { set_error(&e, err_ptr, err_len) };
            STATUS_DECODE_FAILED
        }
    }
}

unsafe fn finish_serialize(
    result: Result<Vec<u8>, String>,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
    err_ptr: *mut *mut u8,
    err_len: *mut usize,
) -> i32 {
    match result {
        Ok(bytes) => {
            let mut bytes = bytes.into_boxed_slice();
            let len = bytes.len();
            let ptr = bytes.as_mut_ptr();
            std::mem::forget(bytes);
            unsafe {
                *out_ptr = ptr;
                *out_len = len;
            }
            STATUS_OK
        }
        Err(e) => {
            unsafe { set_error(&e, err_ptr, err_len) };
            STATUS_ENCODE_FAILED
        }
    }
}

/// Parse `data` into a flat node array. Returns 0 on success.
///
/// On success `*out_ptr`/`*out_len` describe an array to be released with
/// [`tav_cbor_nodes_free`]. String nodes borrow from `data`, which must outlive
/// any use of them.
///
/// # Safety
/// `data` must be valid for `data_len` bytes. All output pointers must be
/// valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_parse_nondet(
    data: *const u8,
    data_len: usize,
    max_depth: usize,
    out_ptr: *mut *mut CborNode,
    out_len: *mut usize,
    err_ptr: *mut *mut u8,
    err_len: *mut usize,
) -> i32 {
    let result = std::panic::catch_unwind(|| {
        let input = unsafe { input_slice(data, data_len) };
        parse_nondet(input, max_depth)
    });

    match result {
        Ok(parsed) => unsafe { finish_parse(parsed, out_ptr, out_len, err_ptr, err_len) },
        Err(_) => {
            unsafe { set_error("panic during tav_cbor_parse_nondet", err_ptr, err_len) };
            STATUS_DECODE_FAILED
        }
    }
}

/// Deterministic (RFC 8949 Section 4.2.1) equivalent of [`tav_cbor_parse_nondet`].
///
/// # Safety
/// See [`tav_cbor_parse_nondet`].
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_parse_det(
    data: *const u8,
    data_len: usize,
    max_depth: usize,
    out_ptr: *mut *mut CborNode,
    out_len: *mut usize,
    err_ptr: *mut *mut u8,
    err_len: *mut usize,
) -> i32 {
    let result = std::panic::catch_unwind(|| {
        let input = unsafe { input_slice(data, data_len) };
        parse_det(input, max_depth)
    });

    match result {
        Ok(parsed) => unsafe { finish_parse(parsed, out_ptr, out_len, err_ptr, err_len) },
        Err(_) => {
            unsafe { set_error("panic during tav_cbor_parse_det", err_ptr, err_len) };
            STATUS_DECODE_FAILED
        }
    }
}

/// Serialize a flat node array. Returns 0 on success.
///
/// On success `*out_ptr`/`*out_len` describe a buffer to be released with
/// [`tav_cbor_buffer_free`].
///
/// # Safety
/// `nodes` must be valid for `nodes_len` entries, and the `ptr`/`len` of any
/// string node must be valid for the duration of the call. All output
/// pointers must be valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_serialize_nondet(
    nodes: *const CborNode,
    nodes_len: usize,
    max_depth: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
    err_ptr: *mut *mut u8,
    err_len: *mut usize,
) -> i32 {
    let result = std::panic::catch_unwind(|| {
        if nodes.is_null() || nodes_len == 0 {
            return Err("No CBOR nodes to serialize".to_string());
        }
        let nodes = unsafe { std::slice::from_raw_parts(nodes, nodes_len) };
        unsafe { serialize_nondet(nodes, max_depth) }
    });

    match result {
        Ok(bytes) => unsafe { finish_serialize(bytes, out_ptr, out_len, err_ptr, err_len) },
        Err(_) => {
            unsafe { set_error("panic during tav_cbor_serialize_nondet", err_ptr, err_len) };
            STATUS_ENCODE_FAILED
        }
    }
}

/// Deterministic (RFC 8949 Section 4.2.1) equivalent of
/// [`tav_cbor_serialize_nondet`], which also sorts map keys.
///
/// # Safety
/// See [`tav_cbor_serialize_nondet`].
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_serialize_det(
    nodes: *const CborNode,
    nodes_len: usize,
    max_depth: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
    err_ptr: *mut *mut u8,
    err_len: *mut usize,
) -> i32 {
    let result = std::panic::catch_unwind(|| {
        if nodes.is_null() || nodes_len == 0 {
            return Err("No CBOR nodes to serialize".to_string());
        }
        let nodes = unsafe { std::slice::from_raw_parts(nodes, nodes_len) };
        unsafe { serialize_det(nodes, max_depth) }
    });

    match result {
        Ok(bytes) => unsafe { finish_serialize(bytes, out_ptr, out_len, err_ptr, err_len) },
        Err(_) => {
            unsafe { set_error("panic during tav_cbor_serialize_det", err_ptr, err_len) };
            STATUS_ENCODE_FAILED
        }
    }
}

/// Release a node array returned by a `tav_cbor_parse_*` call.
///
/// # Safety
/// `ptr`/`len` must come from a successful `tav_cbor_parse_*` call and must not
/// have been released already.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_nodes_free(ptr: *mut CborNode, len: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
}

/// Release a byte buffer or error string returned by any `tav_cbor_*` call.
///
/// # Safety
/// `ptr`/`len` must come from a `tav_cbor_*` call and must not have been released
/// already.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_buffer_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
}
