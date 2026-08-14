// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! C ABI exposing CBOR values as a flat array of [`CborNode`] descriptors.
//!
//! A whole document crosses the boundary in one call. The root is at index 0,
//! and a container's direct children are contiguous, so navigate by following
//! `first_child`. The caller may inspect the array, rebuild it with edits, and
//! serialize it back, in either the deterministic (RFC 8949 Section 4.2.1) or
//! non-deterministic definite-length mode.
//!
//! String payloads are never copied. On parse, the `ptr`/`len` of byte and text
//! nodes point into the caller's input buffer, which must outlive any use of
//! the returned nodes. On serialize, they point into caller-owned storage,
//! which need not be the node array, and which must stay live for the duration
//! of the call. The node array and the serialized buffer are themselves
//! allocated by this crate and released through the `tav_cbor_*_free` calls.

pub mod ffi;

#[cfg(not(target_pointer_width = "64"))]
compile_error!("cbor-ffi exchanges CborNode by layout, which is pinned to 64-bit targets");

use std::borrow::Cow;

use cbor::{CborValue, Det, Mode, Nondet};

/// Node type tags. These must match `TavCborNodeType` in `include/tav/cbor.h`.
pub const CBOR_TYPE_INT: u8 = 0;
pub const CBOR_TYPE_BYTES: u8 = 1;
pub const CBOR_TYPE_TEXT: u8 = 2;
pub const CBOR_TYPE_ARRAY: u8 = 3;
pub const CBOR_TYPE_MAP: u8 = 4;
pub const CBOR_TYPE_TAGGED: u8 = 5;
pub const CBOR_TYPE_SIMPLE: u8 = 6;

/// Status codes. These must match `TavCborStatus` in `include/tav/cbor.h`.
pub const STATUS_OK: i32 = 0;
pub const STATUS_DECODE_FAILED: i32 = 1;
pub const STATUS_ENCODE_FAILED: i32 = 2;

/// Ceiling on the caller-supplied depth, bounding recursion depth so that a
/// cyclic or deeply nested document cannot overflow the stack. Measured at
/// roughly 2.4 KiB of stack per level in debug builds.
pub const MAX_DEPTH_LIMIT: usize = 256;

/// A single CBOR item, as one element of a flat indexed array.
///
/// Mirrors `TavCborNode` in `include/tav/cbor.h`.
///
/// Children of a container occupy `[first_child, first_child + value)` of the
/// same array, and are contiguous. Map children alternate key, value, so a map
/// of `value` entries owns `2 * value` child slots.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CborNode {
    /// One of the `CBOR_TYPE_*` constants.
    pub node_type: u8,
    /// For `CBOR_TYPE_INT`, whether the value is negative.
    pub negative: u8,
    /// Integer magnitude, tag, simple value, or child count.
    pub value: u64,
    /// Byte or text string payload, borrowed from the caller's buffer.
    pub ptr: *const u8,
    pub len: usize,
    pub first_child: usize,
}

/// CBOR encodes a negative integer `n` as the magnitude `-1 - n`. Negating
/// through `u64` keeps `i64::MIN` in range.
fn magnitude_from_int(value: i64) -> (bool, u64) {
    if value < 0 {
        (true, !(value as u64))
    } else {
        (false, value as u64)
    }
}

fn int_from_magnitude(negative: bool, magnitude: u64) -> Result<i64, String> {
    if !negative {
        return i64::try_from(magnitude).map_err(|_| "Failed to decode signed value".to_string());
    }
    if magnitude > i64::MAX as u64 {
        return Err("Failed to decode signed value".to_string());
    }
    Ok(-1 - (magnitude as i64))
}

/// Recover the payload of a string node as a slice of the caller's buffer.
///
/// # Safety
/// `node.ptr` must be valid for `node.len` bytes, which holds for nodes the
/// caller obtained from a parse of a still-live buffer, or built from live C++
/// storage.
unsafe fn node_payload<'a>(node: &CborNode) -> &'a [u8] {
    if node.len == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(node.ptr, node.len) }
}

/// Write `value` into `out[slot]`, appending its descendants.
///
/// A plain preorder walk would interleave grandchildren between siblings, so
/// each container reserves a contiguous block for its own children up front and
/// only then fills it in.
fn emit(value: &CborValue<'_>, out: &mut Vec<CborNode>, slot: usize) {
    match value {
        CborValue::Int(v) => {
            let (negative, magnitude) = magnitude_from_int(*v);
            out[slot] = CborNode {
                node_type: CBOR_TYPE_INT,
                negative: u8::from(negative),
                value: magnitude,
                ..Default::default()
            };
        }
        CborValue::Simple(v) => {
            out[slot] = CborNode {
                node_type: CBOR_TYPE_SIMPLE,
                value: u64::from(*v),
                ..Default::default()
            };
        }
        CborValue::ByteString(payload) => {
            out[slot] = CborNode {
                node_type: CBOR_TYPE_BYTES,
                ptr: payload.as_ptr(),
                len: payload.len(),
                ..Default::default()
            };
        }
        CborValue::TextString(payload) => {
            out[slot] = CborNode {
                node_type: CBOR_TYPE_TEXT,
                ptr: payload.as_ptr(),
                len: payload.len(),
                ..Default::default()
            };
        }
        CborValue::Array(items) => {
            let first_child = out.len();
            out.resize(first_child + items.len(), CborNode::default());
            out[slot] = CborNode {
                node_type: CBOR_TYPE_ARRAY,
                value: items.len() as u64,
                first_child,
                ..Default::default()
            };
            for (i, item) in items.iter().enumerate() {
                emit(item, out, first_child + i);
            }
        }
        CborValue::Map(entries) => {
            let first_child = out.len();
            out.resize(first_child + 2 * entries.len(), CborNode::default());
            out[slot] = CborNode {
                node_type: CBOR_TYPE_MAP,
                value: entries.len() as u64,
                first_child,
                ..Default::default()
            };
            for (i, (key, item)) in entries.iter().enumerate() {
                emit(key, out, first_child + 2 * i);
                emit(item, out, first_child + 2 * i + 1);
            }
        }
        CborValue::Tagged { tag, payload } => {
            let first_child = out.len();
            out.resize(first_child + 1, CborNode::default());
            out[slot] = CborNode {
                node_type: CBOR_TYPE_TAGGED,
                value: *tag,
                first_child,
                ..Default::default()
            };
            emit(payload, out, first_child);
        }
    }
}

/// Parse `input` into a flat node array, borrowing its string payloads.
pub fn parse<M: Mode>(input: &[u8], max_depth: usize) -> Result<Vec<CborNode>, String> {
    let value = CborValue::parse_with_depth::<M>(input, max_depth.min(MAX_DEPTH_LIMIT))?;
    let mut out = vec![CborNode::default()];
    emit(&value, &mut out, 0);
    Ok(out)
}

/// Parse leniently, accepting any well-formed definite-length encoding.
pub fn parse_nondet(input: &[u8], max_depth: usize) -> Result<Vec<CborNode>, String> {
    parse::<Nondet>(input, max_depth)
}

/// Parse strictly, rejecting encodings that are not deterministic.
pub fn parse_det(input: &[u8], max_depth: usize) -> Result<Vec<CborNode>, String> {
    parse::<Det>(input, max_depth)
}

/// Rebuild a value from the flat array, borrowing its string payloads.
///
/// The array is caller-supplied, so every index it carries is bounds checked.
///
/// # Safety
/// String nodes must have valid `ptr`/`len` for the duration of the call.
unsafe fn build<'a>(
    nodes: &[CborNode],
    index: usize,
    depth: usize,
    max_depth: usize,
) -> Result<CborValue<'a>, String> {
    if depth > max_depth {
        return Err(format!("Maximum CBOR nesting depth ({max_depth}) exceeded"));
    }

    let node = nodes
        .get(index)
        .ok_or_else(|| format!("Node index {index} out of bounds"))?;

    let child_count = match node.node_type {
        CBOR_TYPE_ARRAY => usize::try_from(node.value).map_err(|_| "Child count out of range")?,
        CBOR_TYPE_MAP => usize::try_from(node.value)
            .ok()
            .and_then(|count| count.checked_mul(2))
            .ok_or("Child count out of range")?,
        CBOR_TYPE_TAGGED => 1,
        _ => 0,
    };
    // first_child carries no meaning for a childless node, so a caller need
    // not initialise it.
    if child_count > 0 {
        let children_end = node
            .first_child
            .checked_add(child_count)
            .ok_or("Child range overflows")?;
        if children_end > nodes.len() {
            return Err("Child range out of bounds".to_string());
        }
    }

    match node.node_type {
        CBOR_TYPE_INT => Ok(CborValue::Int(int_from_magnitude(
            node.negative != 0,
            node.value,
        )?)),
        CBOR_TYPE_SIMPLE => {
            let value =
                u8::try_from(node.value).map_err(|_| "Simple value out of range".to_string())?;
            Ok(CborValue::Simple(value))
        }
        CBOR_TYPE_BYTES => Ok(CborValue::ByteString(Cow::Borrowed(unsafe {
            node_payload(node)
        }))),
        CBOR_TYPE_TEXT => {
            let payload = unsafe { node_payload(node) };
            let text = std::str::from_utf8(payload)
                .map_err(|_| "Text string is not valid UTF-8".to_string())?;
            Ok(CborValue::TextString(Cow::Borrowed(text)))
        }
        CBOR_TYPE_ARRAY => {
            let mut items = Vec::with_capacity(child_count);
            for i in 0..child_count {
                items.push(unsafe { build(nodes, node.first_child + i, depth + 1, max_depth)? });
            }
            Ok(CborValue::Array(items))
        }
        CBOR_TYPE_MAP => {
            let entry_count = child_count / 2;
            let mut entries = Vec::with_capacity(entry_count);
            for i in 0..entry_count {
                let key = unsafe { build(nodes, node.first_child + 2 * i, depth + 1, max_depth)? };
                let value =
                    unsafe { build(nodes, node.first_child + 2 * i + 1, depth + 1, max_depth)? };
                entries.push((key, value));
            }
            Ok(CborValue::Map(entries))
        }
        CBOR_TYPE_TAGGED => Ok(CborValue::Tagged {
            tag: node.value,
            payload: Box::new(unsafe { build(nodes, node.first_child, depth + 1, max_depth)? }),
        }),
        other => Err(format!("Unknown CBOR node type {other}")),
    }
}

/// Serialize the value rooted at `nodes[0]`.
///
/// # Safety
/// String nodes must have valid `ptr`/`len` for the duration of the call.
pub unsafe fn serialize<M: Mode>(nodes: &[CborNode], max_depth: usize) -> Result<Vec<u8>, String> {
    let max_depth = max_depth.min(MAX_DEPTH_LIMIT);
    let value = unsafe { build(nodes, 0, 0, max_depth)? };
    value.to_bytes_with_depth::<M>(max_depth)
}

/// Serialize preserving map entry order as given.
///
/// # Safety
/// See [`serialize`].
pub unsafe fn serialize_nondet(nodes: &[CborNode], max_depth: usize) -> Result<Vec<u8>, String> {
    unsafe { serialize::<Nondet>(nodes, max_depth) }
}

/// Serialize to deterministic CBOR, sorting map keys.
///
/// # Safety
/// See [`serialize`].
pub unsafe fn serialize_det(nodes: &[CborNode], max_depth: usize) -> Result<Vec<u8>, String> {
    unsafe { serialize::<Det>(nodes, max_depth) }
}

#[cfg(test)]
mod tests;
