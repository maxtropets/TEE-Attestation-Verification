// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#pragma once

#include <stddef.h>
#include <stdint.h>

#define TAV_CBOR_API

#ifdef __cplusplus
extern "C" {
#endif

/*
 * C ABI for bulk CBOR parsing and serialization.
 *
 * A whole document crosses the boundary in a single call, as a flat array of
 * TavCborNode. The root is at index 0, and a container's direct children are
 * contiguous, so navigate by following first_child rather than by walking the
 * array in order. The caller may inspect the array, rebuild it with edits, and
 * serialize it back, choosing deterministic or non-deterministic mode per call.
 *
 * Ownership and lifetime:
 * - String payloads are never copied. On parse, the ptr/len of byte and text
 *   nodes point into the input buffer, which must outlive any use of the
 *   returned array. On serialize, they point into caller-owned storage, which
 *   need not be the node array, and which must stay live for the duration of
 *   the call.
 * - On success, a tav_cbor_parse_* call writes an array through
 *   out_ptr/out_len, to be released with tav_cbor_nodes_free.
 * - On success, a tav_cbor_serialize_* call writes a buffer through
 *   out_ptr/out_len, to be released with tav_cbor_buffer_free.
 * - On failure, if err_ptr/err_len are non-NULL, a UTF-8 message (not NUL
 *   terminated) is written there, also to be released with
 *   tav_cbor_buffer_free. Passing NULL for both suppresses the message.
 * - Freeing a NULL pointer is a no-op.
 * - Output parameters are only written on success, except for the error
 *   message, which is only written on failure.
 *
 * Modes:
 * - The nondet entry points accept any well-formed definite-length encoding
 *   and preserve map entry order on serialization.
 * - The det entry points implement deterministic CBOR per RFC 8949
 *   Section 4.2.1: parsing rejects non-canonical encodings, and serialization
 *   sorts map keys and rejects duplicates.
 * - Both reject floating-point values, indefinite-length encodings, and
 *   invalid UTF-8.
 *
 * Round trips are not byte-preserving in either mode. Integers are decoded to
 * int64_t, so a non-preferred encoding is re-serialized in its preferred form.
 */

/* Status codes. Mirrored by the Rust constants in cbor-ffi/src/lib.rs. */
typedef enum TavCborStatus
{
    TAV_CBOR_OK = 0,
    /* Malformed input, a depth or range violation, or a panic while parsing. */
    TAV_CBOR_DECODE_FAILED = 1,
    /* An unencodable value, a malformed node array, or a panic while
       serializing. */
    TAV_CBOR_ENCODE_FAILED = 2,
} TavCborStatus;

/* Mirrored by the Rust constants in cbor-ffi/src/lib.rs. */
typedef enum TavCborNodeType
{
    TAV_CBOR_NODE_INT = 0,
    TAV_CBOR_NODE_BYTES = 1,
    TAV_CBOR_NODE_TEXT = 2,
    TAV_CBOR_NODE_ARRAY = 3,
    TAV_CBOR_NODE_MAP = 4,
    TAV_CBOR_NODE_TAGGED = 5,
    TAV_CBOR_NODE_SIMPLE = 6,
} TavCborNodeType;

/*
 * A single CBOR item, as one element of a flat indexed array.
 *
 * The root is at index 0. Children of a container occupy
 * [first_child, first_child + value) of the same array and are contiguous.
 * Map children alternate key, value, so a map of value entries owns
 * 2 * value child slots. A tagged item owns exactly one child slot.
 *
 * Child indices are validated on serialization, so an array assembled by the
 * caller cannot read out of bounds.
 */
typedef struct TavCborNode
{
    /* One of the TavCborNodeType values. */
    uint8_t node_type;
    /* For TAV_CBOR_NODE_INT, whether the value is negative. The magnitude in
       value is then the CBOR encoding of the integer, that is -1 - n. */
    uint8_t negative;
    /* Integer magnitude, tag, simple value, or child count. */
    uint64_t value;
    /* Byte or text string payload. Borrowed; never owned by the node. */
    const uint8_t* ptr;
    size_t len;
    size_t first_child;
} TavCborNode;

/*
 * Parse a document into a flat node array, accepting any well-formed
 * definite-length encoding.
 *
 * Rejects trailing bytes, and inputs nested deeper than max_depth.
 * Returns TAV_CBOR_OK, or TAV_CBOR_DECODE_FAILED.
 */
TAV_CBOR_API int tav_cbor_parse_nondet(
  const uint8_t* data,
  size_t data_len,
  size_t max_depth,
  TavCborNode** out_ptr,
  size_t* out_len,
  uint8_t** err_ptr,
  size_t* err_len);

/*
 * Deterministic equivalent of tav_cbor_parse_nondet, which additionally
 * rejects non-canonical encodings.
 */
TAV_CBOR_API int tav_cbor_parse_det(
  const uint8_t* data,
  size_t data_len,
  size_t max_depth,
  TavCborNode** out_ptr,
  size_t* out_len,
  uint8_t** err_ptr,
  size_t* err_len);

/*
 * Serialize the document rooted at nodes[0], preserving map entry order.
 *
 * Rejects values nested deeper than max_depth.
 * Returns TAV_CBOR_OK, or TAV_CBOR_ENCODE_FAILED.
 */
TAV_CBOR_API int tav_cbor_serialize_nondet(
  const TavCborNode* nodes,
  size_t nodes_len,
  size_t max_depth,
  uint8_t** out_ptr,
  size_t* out_len,
  uint8_t** err_ptr,
  size_t* err_len);

/*
 * Deterministic equivalent of tav_cbor_serialize_nondet, which additionally
 * sorts map keys and rejects duplicate keys.
 */
TAV_CBOR_API int tav_cbor_serialize_det(
  const TavCborNode* nodes,
  size_t nodes_len,
  size_t max_depth,
  uint8_t** out_ptr,
  size_t* out_len,
  uint8_t** err_ptr,
  size_t* err_len);

/* Release a node array returned by a tav_cbor_parse_* call. */
TAV_CBOR_API void tav_cbor_nodes_free(TavCborNode* ptr, size_t len);

/* Release a buffer or error message returned by any tav_cbor_* call. */
TAV_CBOR_API void tav_cbor_buffer_free(uint8_t* ptr, size_t len);

#ifdef __cplusplus
}
#endif
