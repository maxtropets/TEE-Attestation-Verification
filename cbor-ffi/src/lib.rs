// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Handle-based C ABI for building, serializing, parsing and inspecting CBOR
//! documents.
//!
//! Consumers use the C++ wrapper in `include/tav/cbor.hpp`, which owns the
//! handles and so upholds the contract below. The ABI itself is declared in
//! `include/tav/detail/cbor_abi.h`.
//!
//! # Handle ownership
//!
//! A handle owns one [`CborValue`] and every child below it. Container
//! constructors consume the handles they are given and null the caller's
//! variables. Each handle in a batch must be distinct: a repeated handle is
//! consumed twice and so freed twice. Callers observing this obtain a tree,
//! which the implementation assumes without validation.
//!
//! # Payload ownership
//!
//! Scalars are copied. Byte and text payloads are borrowed: a handle stores
//! the caller's pointer and length. The `'static` in [`TavCborHandle`] is a
//! claim the caller upholds, since a C handle has no lifetime to name.
//! Buffers passed to a constructor or a parse call must remain alive and
//! unmodified while any handle derived from them is in use.
//!
//! # Limits
//!
//! Parsing and serialization reject nesting deeper than [`MAX_DEPTH_LIMIT`],
//! whatever depth the caller asks for.

#[cfg(all(not(target_family = "wasm"), panic = "abort"))]
compile_error!(
    "tee-attestation-verification-cbor-ffi requires panic = \"unwind\", because its C ABI \
     entry points rely on std::panic::catch_unwind to report a panic as a status code"
);

pub mod ffi;

#[cfg(test)]
mod tests;

use std::borrow::Cow;

use cbor::CborValue;

/// Success.
pub const STATUS_OK: i32 = 0;
/// Malformed input, or a panic while parsing.
pub const STATUS_DECODE_FAILED: i32 = 1;
/// A map key or tag that is not present.
pub const STATUS_KEY_NOT_FOUND: i32 = 2;
/// An index past the end of an array or map.
pub const STATUS_OUT_OF_BOUND: i32 = 3;
/// An operation applied to the wrong kind of value.
pub const STATUS_TYPE_MISMATCH: i32 = 4;
/// An unencodable value, or a panic while serializing.
pub const STATUS_ENCODE_FAILED: i32 = 5;

/// A null or otherwise unreadable handle.
pub const KIND_INVALID: i32 = -1;
pub const KIND_SIGNED: i32 = 0;
pub const KIND_BYTES: i32 = 1;
pub const KIND_STRING: i32 = 2;
pub const KIND_ARRAY: i32 = 3;
pub const KIND_MAP: i32 = 4;
pub const KIND_TAGGED: i32 = 5;
pub const KIND_SIMPLE: i32 = 6;

/// Ceiling on the depth a caller may request, bounding recursion in the
/// parser and serializer so that deeply nested input cannot overflow the
/// stack.
pub const MAX_DEPTH_LIMIT: usize = 256;

/// A CBOR value behind a C handle.
///
/// Byte and text payloads may point into caller memory, which the caller
/// guarantees outlives this value.
#[repr(transparent)]
pub struct TavCborHandle(pub(crate) CborValue<'static>);

/// Move `value` onto the heap and hand the caller an owning handle.
pub(crate) fn into_handle(value: CborValue<'static>) -> *mut TavCborHandle {
    Box::into_raw(Box::new(TavCborHandle(value)))
}

/// Read a handle without taking ownership.
///
/// # Safety
/// `handle` must be null or a live handle.
pub(crate) unsafe fn as_value<'a>(handle: *const TavCborHandle) -> Option<&'a CborValue<'static>> {
    unsafe { handle.as_ref() }.map(|h| &h.0)
}

/// Borrow a child as a handle.
///
/// Sound because [`TavCborHandle`] is `repr(transparent)` over [`CborValue`],
/// so a child's own address serves as its handle.
pub(crate) fn borrow(value: &CborValue<'static>) -> *const TavCborHandle {
    (value as *const CborValue<'static>).cast()
}

/// View caller memory as a slice that outlives this call.
///
/// # Safety
/// `data` must be valid for `len` bytes, and that memory must stay alive and
/// unmodified for as long as any handle built from it is used.
pub(crate) unsafe fn borrowed(data: *const u8, len: usize) -> Option<&'static [u8]> {
    if len == 0 {
        return Some(&[]);
    }
    if data.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(data, len) })
}

/// Take ownership of the handle in `slot`, leaving null behind.
///
/// # Safety
/// `slot` must be null or point to a writable handle variable.
pub(crate) unsafe fn take(slot: *mut *mut TavCborHandle) -> Option<CborValue<'static>> {
    if slot.is_null() {
        return None;
    }
    let handle = unsafe { *slot };
    if handle.is_null() {
        return None;
    }
    unsafe { *slot = std::ptr::null_mut() };
    Some(unsafe { Box::from_raw(handle) }.0)
}

/// Take ownership of `count` handles, returning those already taken if any
/// slot is null.
///
/// Every slot must hold a distinct handle. A handle appearing twice is taken
/// twice and so freed twice, which this does not detect.
///
/// # Safety
/// `slots` must be valid for `count` handle variables holding distinct
/// handles.
pub(crate) unsafe fn take_all(
    slots: *mut *mut TavCborHandle,
    count: usize,
) -> Option<Vec<CborValue<'static>>> {
    if count == 0 {
        return Some(Vec::new());
    }
    if slots.is_null() {
        return None;
    }

    let mut taken = Vec::with_capacity(count);
    for i in 0..count {
        match unsafe { take(slots.add(i)) } {
            Some(value) => taken.push(value),
            None => {
                for (j, value) in taken.into_iter().enumerate() {
                    unsafe { *slots.add(j) = into_handle(value) };
                }
                return None;
            }
        }
    }
    Some(taken)
}

/// Build a byte string that borrows `payload`.
pub(crate) fn bytes_value(payload: &'static [u8]) -> CborValue<'static> {
    CborValue::ByteString(Cow::Borrowed(payload))
}

/// Build a text string that borrows `payload`, rejecting invalid UTF-8.
pub(crate) fn string_value(payload: &'static [u8]) -> Option<CborValue<'static>> {
    std::str::from_utf8(payload)
        .ok()
        .map(|text| CborValue::TextString(Cow::Borrowed(text)))
}

/// Clamp a caller-supplied depth to [`MAX_DEPTH_LIMIT`].
pub(crate) fn capped(max_depth: usize) -> usize {
    max_depth.min(MAX_DEPTH_LIMIT)
}

/// Report the kind of `value` as one of the `KIND_*` constants.
pub(crate) fn kind_of(value: &CborValue<'static>) -> i32 {
    match value {
        CborValue::Int(_) => KIND_SIGNED,
        CborValue::ByteString(_) => KIND_BYTES,
        CborValue::TextString(_) => KIND_STRING,
        CborValue::Array(_) => KIND_ARRAY,
        CborValue::Map(_) => KIND_MAP,
        CborValue::Tagged { .. } => KIND_TAGGED,
        CborValue::Simple(_) => KIND_SIMPLE,
    }
}

/// Whether `value` may be used as a map key.
///
/// Containers are excluded, so that every key a map can hold is also a key
/// the C ABI can look up.
pub(crate) fn usable_as_key(value: &CborValue<'static>) -> bool {
    !matches!(
        value,
        CborValue::Array(_) | CborValue::Map(_) | CborValue::Tagged { .. }
    )
}

/// Whether `value` is a simple value RFC 8949 reserves.
///
/// The reserved range has no encoding, so a handle holding one could be
/// inspected but never serialized.
pub(crate) fn is_reserved_simple(value: u8) -> bool {
    (24..=31).contains(&value)
}
