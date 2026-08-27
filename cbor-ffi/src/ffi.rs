// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! C ABI entry points.
//!
//! Every entry point runs its body under [`std::panic::catch_unwind`], so a
//! panic is reported as a status code or a null handle rather than unwinding
//! into a C frame, which would abort the host process.

use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use cbor::{CborValue, Det, Mode, Nondet};

use crate::{
    as_value, borrow, borrowed, bytes_value, capped, into_handle, is_reserved_simple,
    keys_are_usable, kind_of, string_value, take, take_all, usable_as_key, TavCborHandle,
    KIND_INVALID, STATUS_DECODE_FAILED, STATUS_ENCODE_FAILED, STATUS_KEY_NOT_FOUND, STATUS_OK,
    STATUS_OUT_OF_BOUND, STATUS_TYPE_MISMATCH,
};

/// Run `body`, returning a null handle if it panics.
fn guard_handle(body: impl FnOnce() -> *mut TavCborHandle) -> *mut TavCborHandle {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(std::ptr::null_mut())
}

/// Run `body`, returning `on_panic` if it panics.
fn guard_status(on_panic: i32, body: impl FnOnce() -> i32) -> i32 {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(on_panic)
}

/// Validate and reset a scalar out-parameter before fallible work.
///
/// # Safety
/// `out` must be null or valid for writing.
unsafe fn scalar_out_ptr<T: Default>(out: *mut T) -> bool {
    if out.is_null() {
        return false;
    }
    unsafe { *out = T::default() };
    true
}

/// Validate and reset an owned-pointer out-parameter before fallible work.
///
/// # Safety
/// `out` must be null or valid for writing.
unsafe fn owned_out_ptr<T>(out: *mut *mut T) -> bool {
    if out.is_null() {
        return false;
    }
    unsafe { *out = std::ptr::null_mut() };
    true
}

/// Reset the error out-parameters, which the caller may omit.
///
/// # Safety
/// `err_ptr` and `err_len` must be null or valid for writing.
unsafe fn reset_error(err_ptr: *mut *mut u8, err_len: *mut usize) {
    unsafe {
        owned_out_ptr(err_ptr);
        scalar_out_ptr(err_len);
    }
}

/// Copy `msg` into a caller-owned buffer, released with [`tav_cbor_buffer_free`].
///
/// # Safety
/// `err_ptr` and `err_len` must be null or valid for writing.
unsafe fn set_error(msg: &str, err_ptr: *mut *mut u8, err_len: *mut usize) {
    if err_ptr.is_null() || err_len.is_null() {
        return;
    }
    let bytes = msg.as_bytes().to_vec().into_boxed_slice();
    let len = bytes.len();
    let ptr = Box::into_raw(bytes).cast::<u8>();
    unsafe {
        *err_ptr = ptr;
        *err_len = len;
    }
}

// --- Constructors ---

/// Build a signed integer.
#[no_mangle]
pub extern "C" fn tav_cbor_make_signed(value: i64) -> *mut TavCborHandle {
    guard_handle(|| into_handle(CborValue::Int(value)))
}

/// Build a CBOR simple value, such as false, true, or null.
///
/// Returns null for the values RFC 8949 reserves.
#[no_mangle]
pub extern "C" fn tav_cbor_make_simple(value: u8) -> *mut TavCborHandle {
    guard_handle(|| {
        if is_reserved_simple(value) {
            return std::ptr::null_mut();
        }
        into_handle(CborValue::Simple(value))
    })
}

/// Build a byte string that borrows `data`.
///
/// # Safety
/// `data` must be valid for `len` bytes and outlive the returned handle.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_make_bytes(data: *const u8, len: usize) -> *mut TavCborHandle {
    guard_handle(|| match unsafe { borrowed(data, len) } {
        Some(payload) => into_handle(bytes_value(payload)),
        None => std::ptr::null_mut(),
    })
}

/// Build a text string that borrows `data`, which must be valid UTF-8.
///
/// # Safety
/// `data` must be valid for `len` bytes and outlive the returned handle.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_make_string(
    data: *const c_char,
    len: usize,
) -> *mut TavCborHandle {
    guard_handle(|| {
        let Some(payload) = (unsafe { borrowed(data.cast::<u8>(), len) }) else {
            return std::ptr::null_mut();
        };
        match string_value(payload) {
            Some(value) => into_handle(value),
            None => std::ptr::null_mut(),
        }
    })
}

/// Build an array, consuming `count` handles.
///
/// # Safety
/// `items` must be valid for `count` handle variables.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_make_array(
    items: *mut *mut TavCborHandle,
    count: usize,
) -> *mut TavCborHandle {
    guard_handle(|| match unsafe { take_all(items, count) } {
        Some(values) => into_handle(CborValue::Array(values)),
        None => std::ptr::null_mut(),
    })
}

/// Build a map, consuming `2 * pair_count` handles ordered key, value, key, value.
///
/// Keys must not be arrays, maps or tagged values, matching the lookup
/// [`tav_cbor_map_at`] offers.
///
/// # Safety
/// `pairs` must be valid for `2 * pair_count` handle variables.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_make_map(
    pairs: *mut *mut TavCborHandle,
    pair_count: usize,
) -> *mut TavCborHandle {
    guard_handle(|| {
        let Some(total) = pair_count.checked_mul(2) else {
            return std::ptr::null_mut();
        };
        if total != 0 && pairs.is_null() {
            return std::ptr::null_mut();
        }
        // Checked before anything is consumed, so a rejected batch leaves the
        // caller's handles intact.
        for i in (0..total).step_by(2) {
            match unsafe { as_value(*pairs.add(i)) } {
                Some(key) if !usable_as_key(key) => return std::ptr::null_mut(),
                _ => {}
            }
        }
        let Some(values) = (unsafe { take_all(pairs, total) }) else {
            return std::ptr::null_mut();
        };
        let mut entries = Vec::with_capacity(pair_count);
        let mut it = values.into_iter();
        while let (Some(key), Some(value)) = (it.next(), it.next()) {
            entries.push((key, value));
        }
        into_handle(CborValue::Map(entries))
    })
}

/// Build a tagged value, consuming the payload handle.
///
/// # Safety
/// `payload` must point to a writable handle variable.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_make_tagged(
    tag: u64,
    payload: *mut *mut TavCborHandle,
) -> *mut TavCborHandle {
    guard_handle(|| match unsafe { take(payload) } {
        Some(value) => into_handle(CborValue::Tagged {
            tag,
            payload: Box::new(value),
        }),
        None => std::ptr::null_mut(),
    })
}

/// Copy a value and everything below it.
///
/// Each payload keeps the ownership the source had: a borrowed payload is
/// borrowed again from the same buffer, and an owned payload is copied.
///
/// # Safety
/// `value` must be null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_shallow_copy(value: *const TavCborHandle) -> *mut TavCborHandle {
    guard_handle(|| match unsafe { as_value(value) } {
        Some(value) => into_handle(value.clone()),
        None => std::ptr::null_mut(),
    })
}

/// Copy a value and everything below it, copying every payload.
///
/// The result borrows nothing, so it outlives the buffers the source was
/// built over.
///
/// # Safety
/// `value` must be null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_deep_copy(value: *const TavCborHandle) -> *mut TavCborHandle {
    guard_handle(|| match unsafe { as_value(value) } {
        Some(value) => into_handle(value.clone().into_owned()),
        None => std::ptr::null_mut(),
    })
}

/// Release an owning handle. Freeing null is a no-op.
///
/// # Safety
/// `value` must come from a constructor or a parse call, and must not have
/// been released already. Borrowed handles must not be passed here.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_free(value: *mut TavCborHandle) {
    if value.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(value) })));
}

// --- Serialization ---

unsafe fn serialize<M: Mode>(
    value: *const TavCborHandle,
    max_depth: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
    err_ptr: *mut *mut u8,
    err_len: *mut usize,
) -> i32 {
    unsafe { reset_error(err_ptr, err_len) };
    let out_ok = unsafe { owned_out_ptr(out_ptr) };
    let len_ok = unsafe { scalar_out_ptr(out_len) };
    let Some(value) = (unsafe { as_value(value) }) else {
        unsafe { set_error("Null CBOR handle", err_ptr, err_len) };
        return STATUS_ENCODE_FAILED;
    };
    if !out_ok || !len_ok {
        unsafe { set_error("Null output pointer", err_ptr, err_len) };
        return STATUS_ENCODE_FAILED;
    }
    match value.to_bytes_with_depth::<M>(capped(max_depth)) {
        Ok(bytes) => {
            let bytes = bytes.into_boxed_slice();
            let len = bytes.len();
            let ptr = Box::into_raw(bytes).cast::<u8>();
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

/// Serialize.
///
/// The buffer and message outputs are cleared before any work, so a failure
/// leaves no stale pointer to free. On success writes an owned buffer through
/// `out_ptr`/`out_len`, released with [`tav_cbor_buffer_free`]. On failure
/// writes a UTF-8 message, not NUL terminated, through `err_ptr`/`err_len`,
/// released the same way.
///
/// # Safety
/// All output pointers must be null or valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_nondet_serialize(
    value: *const TavCborHandle,
    max_depth: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
    err_ptr: *mut *mut u8,
    err_len: *mut usize,
) -> i32 {
    guard_status(STATUS_ENCODE_FAILED, || unsafe {
        serialize::<Nondet>(value, max_depth, out_ptr, out_len, err_ptr, err_len)
    })
}

/// Serialize with deterministic encoding.
///
/// # Safety
/// All output pointers must be null or valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_det_serialize(
    value: *const TavCborHandle,
    max_depth: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
    err_ptr: *mut *mut u8,
    err_len: *mut usize,
) -> i32 {
    guard_status(STATUS_ENCODE_FAILED, || unsafe {
        serialize::<Det>(value, max_depth, out_ptr, out_len, err_ptr, err_len)
    })
}

// --- Parsing ---

unsafe fn parse<M: Mode>(
    data: *const u8,
    len: usize,
    max_depth: usize,
    out_value: *mut *mut TavCborHandle,
    err_ptr: *mut *mut u8,
    err_len: *mut usize,
) -> i32 {
    unsafe { reset_error(err_ptr, err_len) };
    if !unsafe { owned_out_ptr(out_value) } {
        unsafe { set_error("Null output pointer", err_ptr, err_len) };
        return STATUS_DECODE_FAILED;
    }
    let Some(bytes) = (unsafe { borrowed(data, len) }) else {
        unsafe { set_error("Null CBOR input", err_ptr, err_len) };
        return STATUS_DECODE_FAILED;
    };
    match CborValue::parse_with_depth::<M>(bytes, capped(max_depth)) {
        Ok(value) => {
            if !keys_are_usable(&value) {
                unsafe { set_error("Container used as a map key", err_ptr, err_len) };
                return STATUS_DECODE_FAILED;
            }
            unsafe { *out_value = into_handle(value) };
            STATUS_OK
        }
        Err(e) => {
            unsafe { set_error(&e, err_ptr, err_len) };
            STATUS_DECODE_FAILED
        }
    }
}

/// Parse.
///
/// Indefinite-length encodings are rejected, and the whole input must be
/// consumed. A document that keys a map entry on a container is rejected too,
/// so a parsed map holds only keys [`tav_cbor_map_at`] can look up. The
/// returned tree borrows byte and text payloads from `data`, which must
/// outlive it. The handle and message outputs are cleared before any work, so
/// a failure leaves no stale handle to free.
///
/// # Safety
/// `data` must be valid for `len` bytes and outlive the returned handle. All
/// output pointers must be null or valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_nondet_parse(
    data: *const u8,
    len: usize,
    max_depth: usize,
    out_value: *mut *mut TavCborHandle,
    err_ptr: *mut *mut u8,
    err_len: *mut usize,
) -> i32 {
    guard_status(STATUS_DECODE_FAILED, || unsafe {
        parse::<Nondet>(data, len, max_depth, out_value, err_ptr, err_len)
    })
}

/// Parse, requiring deterministic encoding.
///
/// # Safety
/// `data` must be valid for `len` bytes and outlive the returned handle. All
/// output pointers must be null or valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_det_parse(
    data: *const u8,
    len: usize,
    max_depth: usize,
    out_value: *mut *mut TavCborHandle,
    err_ptr: *mut *mut u8,
    err_len: *mut usize,
) -> i32 {
    guard_status(STATUS_DECODE_FAILED, || unsafe {
        parse::<Det>(data, len, max_depth, out_value, err_ptr, err_len)
    })
}

/// Release a buffer or error message. Freeing null is a no-op.
///
/// # Safety
/// `ptr`/`len` must come from a `tav_cbor_*` call and must not have been
/// released already.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_buffer_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
}

// --- Inspection ---

/// Report the kind of `value`, or `KIND_INVALID` for a null handle.
///
/// # Safety
/// `value` must be null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_kind(value: *const TavCborHandle) -> i32 {
    guard_status(KIND_INVALID, || match unsafe { as_value(value) } {
        Some(value) => kind_of(value),
        None => KIND_INVALID,
    })
}

/// Read a signed integer.
///
/// # Safety
/// `value` must be null or a live handle, and `out` valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_as_signed(value: *const TavCborHandle, out: *mut i64) -> i32 {
    guard_status(STATUS_TYPE_MISMATCH, || {
        if out.is_null() {
            return STATUS_TYPE_MISMATCH;
        }
        match unsafe { as_value(value) } {
            Some(CborValue::Int(v)) => {
                unsafe { *out = *v };
                STATUS_OK
            }
            _ => STATUS_TYPE_MISMATCH,
        }
    })
}

/// Read a simple value.
///
/// # Safety
/// `value` must be null or a live handle, and `out` valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_as_simple(value: *const TavCborHandle, out: *mut u8) -> i32 {
    guard_status(STATUS_TYPE_MISMATCH, || {
        if out.is_null() {
            return STATUS_TYPE_MISMATCH;
        }
        match unsafe { as_value(value) } {
            Some(CborValue::Simple(v)) => {
                unsafe { *out = *v };
                STATUS_OK
            }
            _ => STATUS_TYPE_MISMATCH,
        }
    })
}

/// Read a byte string payload, which points into the buffer it borrows.
///
/// # Safety
/// `value` must be null or a live handle, and the outputs valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_as_bytes(
    value: *const TavCborHandle,
    out: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    guard_status(STATUS_TYPE_MISMATCH, || {
        if out.is_null() || out_len.is_null() {
            return STATUS_TYPE_MISMATCH;
        }
        match unsafe { as_value(value) } {
            Some(CborValue::ByteString(payload)) => {
                unsafe {
                    *out = payload.as_ptr();
                    *out_len = payload.len();
                }
                STATUS_OK
            }
            _ => STATUS_TYPE_MISMATCH,
        }
    })
}

/// Read a text string payload, which points into the buffer it borrows.
///
/// # Safety
/// `value` must be null or a live handle, and the outputs valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_as_string(
    value: *const TavCborHandle,
    out: *mut *const c_char,
    out_len: *mut usize,
) -> i32 {
    guard_status(STATUS_TYPE_MISMATCH, || {
        if out.is_null() || out_len.is_null() {
            return STATUS_TYPE_MISMATCH;
        }
        match unsafe { as_value(value) } {
            Some(CborValue::TextString(payload)) => {
                unsafe {
                    *out = payload.as_ptr().cast();
                    *out_len = payload.len();
                }
                STATUS_OK
            }
            _ => STATUS_TYPE_MISMATCH,
        }
    })
}

/// Read the tag of a tagged value.
///
/// # Safety
/// `value` must be null or a live handle, and `out` valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_as_tag(value: *const TavCborHandle, out: *mut u64) -> i32 {
    guard_status(STATUS_TYPE_MISMATCH, || {
        if out.is_null() {
            return STATUS_TYPE_MISMATCH;
        }
        match unsafe { as_value(value) } {
            Some(CborValue::Tagged { tag, .. }) => {
                unsafe { *out = *tag };
                STATUS_OK
            }
            _ => STATUS_TYPE_MISMATCH,
        }
    })
}

/// Read the entry count of an array or map.
///
/// # Safety
/// `value` must be null or a live handle, and `out` valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_size(value: *const TavCborHandle, out: *mut usize) -> i32 {
    guard_status(STATUS_TYPE_MISMATCH, || {
        if out.is_null() {
            return STATUS_TYPE_MISMATCH;
        }
        let count = match unsafe { as_value(value) } {
            Some(CborValue::Array(items)) => items.len(),
            Some(CborValue::Map(entries)) => entries.len(),
            _ => return STATUS_TYPE_MISMATCH,
        };
        unsafe { *out = count };
        STATUS_OK
    })
}

// --- Navigation ---

/// Borrow an array element by index.
///
/// # Safety
/// `value` must be null or a live handle, and `out` valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_array_at(
    value: *const TavCborHandle,
    index: usize,
    out: *mut *const TavCborHandle,
) -> i32 {
    guard_status(STATUS_TYPE_MISMATCH, || {
        if out.is_null() {
            return STATUS_TYPE_MISMATCH;
        }
        let Some(CborValue::Array(items)) = (unsafe { as_value(value) }) else {
            return STATUS_TYPE_MISMATCH;
        };
        match items.get(index) {
            Some(item) => {
                unsafe { *out = borrow(item) };
                STATUS_OK
            }
            None => STATUS_OUT_OF_BOUND,
        }
    })
}

/// Borrow a map value by key.
///
/// Containers are not usable as keys and are reported as a type mismatch.
///
/// # Safety
/// `value` and `key` must be null or live handles, and `out` valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_map_at(
    value: *const TavCborHandle,
    key: *const TavCborHandle,
    out: *mut *const TavCborHandle,
) -> i32 {
    guard_status(STATUS_TYPE_MISMATCH, || {
        if out.is_null() {
            return STATUS_TYPE_MISMATCH;
        }
        let Some(CborValue::Map(entries)) = (unsafe { as_value(value) }) else {
            return STATUS_TYPE_MISMATCH;
        };
        let Some(key) = (unsafe { as_value(key) }) else {
            return STATUS_TYPE_MISMATCH;
        };
        if !usable_as_key(key) {
            return STATUS_TYPE_MISMATCH;
        }
        match entries.iter().find(|(k, _)| k == key) {
            Some((_, found)) => {
                unsafe { *out = borrow(found) };
                STATUS_OK
            }
            None => STATUS_KEY_NOT_FOUND,
        }
    })
}

/// Borrow the payload of a tagged value, checking the tag.
///
/// # Safety
/// `value` must be null or a live handle, and `out` valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_tag_at(
    value: *const TavCborHandle,
    tag: u64,
    out: *mut *const TavCborHandle,
) -> i32 {
    guard_status(STATUS_TYPE_MISMATCH, || {
        if out.is_null() {
            return STATUS_TYPE_MISMATCH;
        }
        let Some(CborValue::Tagged {
            tag: actual,
            payload,
        }) = (unsafe { as_value(value) })
        else {
            return STATUS_TYPE_MISMATCH;
        };
        if *actual != tag {
            return STATUS_KEY_NOT_FOUND;
        }
        unsafe { *out = borrow(payload) };
        STATUS_OK
    })
}

/// Borrow a map key by entry index.
///
/// # Safety
/// `value` must be null or a live handle, and `out` valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_map_key_at(
    value: *const TavCborHandle,
    index: usize,
    out: *mut *const TavCborHandle,
) -> i32 {
    guard_status(STATUS_TYPE_MISMATCH, || unsafe {
        map_entry_at(value, index, out, true)
    })
}

/// Borrow a map value by entry index.
///
/// # Safety
/// `value` must be null or a live handle, and `out` valid for writing.
#[no_mangle]
pub unsafe extern "C" fn tav_cbor_map_value_at(
    value: *const TavCborHandle,
    index: usize,
    out: *mut *const TavCborHandle,
) -> i32 {
    guard_status(STATUS_TYPE_MISMATCH, || unsafe {
        map_entry_at(value, index, out, false)
    })
}

unsafe fn map_entry_at(
    value: *const TavCborHandle,
    index: usize,
    out: *mut *const TavCborHandle,
    want_key: bool,
) -> i32 {
    if out.is_null() {
        return STATUS_TYPE_MISMATCH;
    }
    let Some(CborValue::Map(entries)) = (unsafe { as_value(value) }) else {
        return STATUS_TYPE_MISMATCH;
    };
    match entries.get(index) {
        Some((key, item)) => {
            unsafe { *out = borrow(if want_key { key } else { item }) };
            STATUS_OK
        }
        None => STATUS_OUT_OF_BOUND,
    }
}
