// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;

fn round_trip_nondet(encoded: &[u8]) -> Vec<u8> {
    let nodes = parse_nondet(encoded, 16).unwrap();
    unsafe { serialize_nondet(&nodes, 16) }.unwrap()
}

#[test]
fn round_trips_scalars() {
    // 0, 1, -1, -7, simple(true), simple(null)
    for encoded in [
        vec![0x00],
        vec![0x01],
        vec![0x20],
        vec![0x26],
        vec![0xf5],
        vec![0xf6],
    ] {
        assert_eq!(round_trip_nondet(&encoded), encoded);
    }
}

#[test]
fn round_trips_strings() {
    // "", "a", h'', h'0102'
    for encoded in [
        vec![0x60],
        vec![0x61, 0x61],
        vec![0x40],
        vec![0x42, 0x01, 0x02],
    ] {
        assert_eq!(round_trip_nondet(&encoded), encoded);
    }
}

#[test]
fn round_trips_nested_containers() {
    // [1, [2, 3], {1: "a"}], and tag(18, [])
    for encoded in [
        vec![0x83, 0x01, 0x82, 0x02, 0x03, 0xa1, 0x01, 0x61, 0x61],
        vec![0xd2, 0x80],
    ] {
        assert_eq!(round_trip_nondet(&encoded), encoded);
    }
}

#[test]
fn parse_reports_trailing_bytes() {
    assert!(parse_nondet(&[0x00, 0x00], 16)
        .unwrap_err()
        .contains("Trailing bytes"));
}

#[test]
fn parse_rejects_garbage() {
    assert!(parse_nondet(&[0xff], 16).is_err());
}

#[test]
fn parse_enforces_max_depth() {
    // [[[[1]]]] nested 4 deep, rejected with max_depth 2.
    let encoded = vec![0x81, 0x81, 0x81, 0x81, 0x01];
    assert!(parse_nondet(&encoded, 2)
        .unwrap_err()
        .contains("Maximum CBOR nesting depth"));
    assert!(parse_nondet(&encoded, 16).is_ok());
}

#[test]
fn serialize_enforces_max_depth() {
    let encoded = vec![0x81, 0x81, 0x81, 0x81, 0x01];
    let nodes = parse_nondet(&encoded, 16).unwrap();
    assert!(unsafe { serialize_nondet(&nodes, 2) }
        .unwrap_err()
        .contains("Maximum CBOR nesting depth"));
    assert!(unsafe { serialize_nondet(&nodes, 16) }.is_ok());
}

#[test]
fn parse_borrows_from_input() {
    let encoded = vec![0x42, 0xaa, 0xbb];
    let nodes = parse_nondet(&encoded, 16).unwrap();
    assert_eq!(nodes[0].node_type, CBOR_TYPE_BYTES);
    assert_eq!(nodes[0].len, 2);
    // The payload must point into the caller's buffer, not a copy.
    assert_eq!(nodes[0].ptr, encoded[1..].as_ptr());
}

#[test]
fn map_children_are_contiguous_key_value_pairs() {
    // {1: 2, 3: 4}
    let nodes = parse_nondet(&[0xa2, 0x01, 0x02, 0x03, 0x04], 16).unwrap();
    assert_eq!(nodes[0].node_type, CBOR_TYPE_MAP);
    assert_eq!(nodes[0].value, 2);
    let first = nodes[0].first_child;
    assert_eq!(nodes[first].value, 1);
    assert_eq!(nodes[first + 1].value, 2);
    assert_eq!(nodes[first + 2].value, 3);
    assert_eq!(nodes[first + 3].value, 4);
}

/// Grandchildren must not be interleaved between siblings.
#[test]
fn sibling_children_stay_contiguous_with_nested_containers() {
    // [[1, 2], [3, 4]]
    let nodes = parse_nondet(&[0x82, 0x82, 0x01, 0x02, 0x82, 0x03, 0x04], 16).unwrap();
    assert_eq!(nodes[0].node_type, CBOR_TYPE_ARRAY);
    let outer = nodes[0].first_child;
    for slot in [outer, outer + 1] {
        assert_eq!(nodes[slot].node_type, CBOR_TYPE_ARRAY);
        assert_eq!(nodes[slot].value, 2);
    }
    let left = nodes[outer].first_child;
    assert_eq!(nodes[left].value, 1);
    assert_eq!(nodes[left + 1].value, 2);
    let right = nodes[outer + 1].first_child;
    assert_eq!(nodes[right].value, 3);
    assert_eq!(nodes[right + 1].value, 4);
}

#[test]
fn negative_integers_survive_round_trip() {
    let nodes = parse_nondet(&[0x20], 16).unwrap();
    assert_eq!(nodes[0].negative, 1);
    assert_eq!(nodes[0].value, 0);

    // -(2^32), which is already in preferred form.
    let large = vec![0x3a, 0xff, 0xff, 0xff, 0xff];
    assert_eq!(round_trip_nondet(&large), large);

    // i64::MIN, the boundary that must negate through u64.
    let min = vec![0x3b, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    assert_eq!(round_trip_nondet(&min), min);
}

#[test]
fn non_preferred_integer_encodings_are_normalised() {
    // -2 encoded in the non-preferred 8 byte form. Parsing discards the
    // original width, so re-serializing yields the preferred form. Round trips
    // are therefore not byte-preserving.
    let non_preferred = vec![0x3b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01];
    assert_eq!(round_trip_nondet(&non_preferred), vec![0x21]);
}

#[test]
fn nondet_preserves_map_order_and_det_sorts_it() {
    // {2: 0, 1: 0} is valid non-deterministic CBOR but not canonical.
    let encoded = vec![0xa2, 0x02, 0x00, 0x01, 0x00];
    assert_eq!(round_trip_nondet(&encoded), encoded);

    // Deterministic serialization sorts the keys.
    let nodes = parse_nondet(&encoded, 16).unwrap();
    let det_bytes = unsafe { serialize_det(&nodes, 16) }.unwrap();
    assert_eq!(det_bytes, vec![0xa2, 0x01, 0x00, 0x02, 0x00]);

    // And deterministic parsing rejects the non-canonical ordering.
    assert!(parse_det(&encoded, 16).is_err());
}

#[test]
fn serialize_rejects_out_of_bounds_child_indices() {
    // An array claiming a child that the node array does not contain.
    let nodes = vec![CborNode {
        node_type: CBOR_TYPE_ARRAY,
        value: 1,
        first_child: 9,
        ..Default::default()
    }];
    assert!(unsafe { serialize_nondet(&nodes, 16) }
        .unwrap_err()
        .contains("out of bounds"));
}

#[test]
fn serialize_rejects_unknown_node_types() {
    let nodes = vec![CborNode {
        node_type: 42,
        ..Default::default()
    }];
    assert!(unsafe { serialize_nondet(&nodes, 16) }
        .unwrap_err()
        .contains("Unknown CBOR node type"));
}

// --- C header synchronisation ---

/// Parse `NAME = VALUE,` enumerator lines out of the C header.
fn c_header_enum_value(header: &str, name: &str) -> Option<i64> {
    header.lines().find_map(|line| {
        let (lhs, rhs) = line.split_once('=')?;
        // Token-exact, so a name that is a prefix of another cannot match.
        if lhs.trim() != name {
            return None;
        }
        rhs.trim().trim_end_matches(',').parse().ok()
    })
}

fn c_header_names(header: &str, prefix: &str) -> std::collections::BTreeSet<String> {
    header
        .lines()
        .filter_map(|line| {
            let name = line.trim().split_once('=')?.0.trim().to_string();
            name.starts_with(prefix).then_some(name)
        })
        .collect()
}

#[test]
fn c_header_node_types_match_rust() {
    let header = include_str!("../include/tav/cbor.h");
    let map = [
        ("TAV_CBOR_NODE_INT", CBOR_TYPE_INT),
        ("TAV_CBOR_NODE_BYTES", CBOR_TYPE_BYTES),
        ("TAV_CBOR_NODE_TEXT", CBOR_TYPE_TEXT),
        ("TAV_CBOR_NODE_ARRAY", CBOR_TYPE_ARRAY),
        ("TAV_CBOR_NODE_MAP", CBOR_TYPE_MAP),
        ("TAV_CBOR_NODE_TAGGED", CBOR_TYPE_TAGGED),
        ("TAV_CBOR_NODE_SIMPLE", CBOR_TYPE_SIMPLE),
    ];
    for (name, value) in map {
        assert_eq!(
            c_header_enum_value(header, name),
            Some(i64::from(value)),
            "{name} in include/tav/cbor.h must match the Rust constant"
        );
    }

    // Completeness in both directions: a node type added to the header without
    // a Rust constant, or vice versa, fails here.
    let declared = c_header_names(header, "TAV_CBOR_NODE_");
    let checked: std::collections::BTreeSet<String> =
        map.iter().map(|(name, _)| (*name).to_string()).collect();
    assert_eq!(declared, checked);
}

#[test]
fn c_header_status_codes_match_rust() {
    let header = include_str!("../include/tav/cbor.h");
    let map = [
        ("TAV_CBOR_OK", STATUS_OK),
        ("TAV_CBOR_DECODE_FAILED", STATUS_DECODE_FAILED),
        ("TAV_CBOR_ENCODE_FAILED", STATUS_ENCODE_FAILED),
    ];
    for (name, value) in map {
        assert_eq!(
            c_header_enum_value(header, name),
            Some(i64::from(value)),
            "{name} in include/tav/cbor.h must match the Rust constant"
        );
    }

    let declared: std::collections::BTreeSet<String> = c_header_names(header, "TAV_CBOR_")
        .into_iter()
        .filter(|name| !name.starts_with("TAV_CBOR_NODE_"))
        .collect();
    let checked: std::collections::BTreeSet<String> =
        map.iter().map(|(name, _)| (*name).to_string()).collect();
    assert_eq!(declared, checked);
}

#[test]
fn c_header_declares_every_exported_symbol() {
    let header = include_str!("../include/tav/cbor.h");
    let ffi = include_str!("ffi.rs");

    let exported: std::collections::BTreeSet<&str> = ffi
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("pub unsafe extern \"C\" fn ")?;
            rest.split('(').next()
        })
        .collect();
    assert_eq!(exported.len(), 6, "expected six entry points");

    for symbol in exported {
        assert!(
            header.contains(symbol),
            "{symbol} is exported but not declared in include/tav/cbor.h"
        );
    }
}

// --- C ABI layout ---

/// The node array crosses the ABI by raw layout, so these numbers are also
/// asserted from the C side.
#[test]
fn cbor_node_layout_is_pinned() {
    use std::mem::{align_of, offset_of, size_of};

    assert_eq!(size_of::<CborNode>(), 40);
    assert_eq!(align_of::<CborNode>(), 8);
    assert_eq!(offset_of!(CborNode, node_type), 0);
    assert_eq!(offset_of!(CborNode, negative), 1);
    assert_eq!(offset_of!(CborNode, value), 8);
    assert_eq!(offset_of!(CborNode, ptr), 16);
    assert_eq!(offset_of!(CborNode, len), 24);
    assert_eq!(offset_of!(CborNode, first_child), 32);
}

#[test]
fn childless_nodes_ignore_first_child() {
    // first_child carries no meaning without children, so a caller may leave
    // it uninitialised.
    for node_type in [
        CBOR_TYPE_INT,
        CBOR_TYPE_SIMPLE,
        CBOR_TYPE_BYTES,
        CBOR_TYPE_TEXT,
    ] {
        let nodes = vec![CborNode {
            node_type,
            first_child: usize::MAX,
            ..Default::default()
        }];
        assert!(
            unsafe { serialize_nondet(&nodes, 16) }.is_ok(),
            "type {node_type} should ignore first_child"
        );
    }

    // Empty containers have no children either.
    for node_type in [CBOR_TYPE_ARRAY, CBOR_TYPE_MAP] {
        let nodes = vec![CborNode {
            node_type,
            value: 0,
            first_child: usize::MAX,
            ..Default::default()
        }];
        assert!(unsafe { serialize_nondet(&nodes, 16) }.is_ok());
    }
}

#[test]
fn oversized_child_counts_are_rejected_without_overflow() {
    // 2 * value overflows usize for a map.
    let nodes = vec![CborNode {
        node_type: CBOR_TYPE_MAP,
        value: u64::MAX,
        first_child: 1,
        ..Default::default()
    }];
    assert!(unsafe { serialize_nondet(&nodes, 16) }
        .unwrap_err()
        .contains("Child count out of range"));

    // An array count that no node array can back.
    let nodes = vec![CborNode {
        node_type: CBOR_TYPE_ARRAY,
        value: u64::MAX,
        first_child: 1,
        ..Default::default()
    }];
    assert!(unsafe { serialize_nondet(&nodes, 16) }
        .unwrap_err()
        .contains("Child range overflows"));

    // A count that fits, but that the node array does not back.
    let nodes = vec![CborNode {
        node_type: CBOR_TYPE_ARRAY,
        value: 4,
        first_child: 1,
        ..Default::default()
    }];
    assert!(unsafe { serialize_nondet(&nodes, 16) }
        .unwrap_err()
        .contains("out of bounds"));
}

#[test]
fn depth_is_capped_regardless_of_the_requested_maximum() {
    // An array whose only child is itself. Without a ceiling on the caller's
    // max_depth this recurses until the stack overflows.
    let nodes = vec![CborNode {
        node_type: CBOR_TYPE_ARRAY,
        value: 1,
        first_child: 0,
        ..Default::default()
    }];
    let err = unsafe { serialize_nondet(&nodes, usize::MAX) }.expect_err("cycle should fail");
    assert!(err.contains(&MAX_DEPTH_LIMIT.to_string()), "{err}");
}
