// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Consumer tests for the bulk CBOR C ABI (tav/cbor.h). These link the built
// library and drive the exported symbols through the installed header, as an
// external C consumer would.

#include "support.h"

#include <cstddef>
#include <cstring>
#include <limits>

extern "C" {
#include "tav/cbor.h"
}

namespace {

// The node array crosses the ABI by raw layout, so the C view of TavCborNode
// must match the Rust #[repr(C)] CborNode, which asserts the same numbers.
static_assert(sizeof(TavCborNode) == 40, "TavCborNode size");
static_assert(alignof(TavCborNode) == 8, "TavCborNode alignment");
static_assert(offsetof(TavCborNode, node_type) == 0, "node_type offset");
static_assert(offsetof(TavCborNode, negative) == 1, "negative offset");
static_assert(offsetof(TavCborNode, value) == 8, "value offset");
static_assert(offsetof(TavCborNode, ptr) == 16, "ptr offset");
static_assert(offsetof(TavCborNode, len) == 24, "len offset");
static_assert(offsetof(TavCborNode, first_child) == 32, "first_child offset");

constexpr size_t kMaxDepth = 16;

// Owns a node array returned by a tav_cbor_parse_* call.
struct Nodes {
    TavCborNode* ptr = nullptr;
    size_t len = 0;

    Nodes() = default;
    Nodes(const Nodes&) = delete;
    Nodes& operator=(const Nodes&) = delete;
    ~Nodes() { tav_cbor_nodes_free(ptr, len); }

    const TavCborNode& operator[](size_t index) const {
        REQUIRE(index < len);
        return ptr[index];
    }
};

// Owns a buffer or error message returned by any tav_cbor_* call.
struct Buffer {
    uint8_t* ptr = nullptr;
    size_t len = 0;

    Buffer() = default;
    Buffer(const Buffer&) = delete;
    Buffer& operator=(const Buffer&) = delete;
    ~Buffer() { tav_cbor_buffer_free(ptr, len); }

    std::vector<uint8_t> bytes() const { return std::vector<uint8_t>(ptr, ptr + len); }
    std::string text() const { return std::string(reinterpret_cast<const char*>(ptr), len); }
};

int parse_nondet(const std::vector<uint8_t>& input, Nodes& nodes, Buffer& err,
                 size_t max_depth = kMaxDepth) {
    return tav_cbor_parse_nondet(input.data(), input.size(), max_depth, &nodes.ptr, &nodes.len,
                                 &err.ptr, &err.len);
}

int parse_det(const std::vector<uint8_t>& input, Nodes& nodes, Buffer& err,
              size_t max_depth = kMaxDepth) {
    return tav_cbor_parse_det(input.data(), input.size(), max_depth, &nodes.ptr, &nodes.len,
                              &err.ptr, &err.len);
}

int serialize_nondet(const std::vector<TavCborNode>& nodes, Buffer& out, Buffer& err,
                     size_t max_depth = kMaxDepth) {
    return tav_cbor_serialize_nondet(nodes.data(), nodes.size(), max_depth, &out.ptr, &out.len,
                                     &err.ptr, &err.len);
}

int serialize_det(const std::vector<TavCborNode>& nodes, Buffer& out, Buffer& err,
                  size_t max_depth = kMaxDepth) {
    return tav_cbor_serialize_det(nodes.data(), nodes.size(), max_depth, &out.ptr, &out.len,
                                  &err.ptr, &err.len);
}

// Copy a parsed array into caller-owned storage so it can be serialized back.
std::vector<TavCborNode> copy_of(const Nodes& nodes) {
    return std::vector<TavCborNode>(nodes.ptr, nodes.ptr + nodes.len);
}

} // namespace

TEST_CASE("cbor C ABI: parse yields a preorder array with contiguous children") {
    // [1, "hi", h'0102']
    const std::vector<uint8_t> input = {0x83, 0x01, 0x62, 0x68, 0x69, 0x42, 0x01, 0x02};
    Nodes nodes;
    Buffer err;
    REQUIRE(parse_nondet(input, nodes, err) == TAV_CBOR_OK);
    REQUIRE(nodes.len == 4);

    CHECK(nodes[0].node_type == TAV_CBOR_NODE_ARRAY);
    CHECK(nodes[0].value == 3);
    const size_t first = nodes[0].first_child;

    CHECK(nodes[first].node_type == TAV_CBOR_NODE_INT);
    CHECK(nodes[first].negative == 0);
    CHECK(nodes[first].value == 1);

    CHECK(nodes[first + 1].node_type == TAV_CBOR_NODE_TEXT);
    CHECK(nodes[first + 1].len == 2);
    CHECK(std::memcmp(nodes[first + 1].ptr, "hi", 2) == 0);

    CHECK(nodes[first + 2].node_type == TAV_CBOR_NODE_BYTES);
    CHECK(nodes[first + 2].len == 2);
}

TEST_CASE("cbor C ABI: string nodes borrow the caller's buffer") {
    const std::vector<uint8_t> input = {0x42, 0xaa, 0xbb};
    Nodes nodes;
    Buffer err;
    REQUIRE(parse_nondet(input, nodes, err) == TAV_CBOR_OK);
    REQUIRE(nodes.len == 1);
    CHECK(nodes[0].node_type == TAV_CBOR_NODE_BYTES);
    CHECK(nodes[0].len == 2);
    // Nothing is copied: the payload points into the input, not a duplicate.
    CHECK(nodes[0].ptr == input.data() + 1);
}

TEST_CASE("cbor C ABI: map children alternate key and value") {
    // {1: 2, 3: 4}
    const std::vector<uint8_t> input = {0xa2, 0x01, 0x02, 0x03, 0x04};
    Nodes nodes;
    Buffer err;
    REQUIRE(parse_nondet(input, nodes, err) == TAV_CBOR_OK);
    CHECK(nodes[0].node_type == TAV_CBOR_NODE_MAP);
    CHECK(nodes[0].value == 2);
    const size_t first = nodes[0].first_child;
    CHECK(nodes[first].value == 1);
    CHECK(nodes[first + 1].value == 2);
    CHECK(nodes[first + 2].value == 3);
    CHECK(nodes[first + 3].value == 4);
}

TEST_CASE("cbor C ABI: tagged items own exactly one child") {
    // 18(h'2a')
    const std::vector<uint8_t> input = {0xd2, 0x41, 0x2a};
    Nodes nodes;
    Buffer err;
    REQUIRE(parse_nondet(input, nodes, err) == TAV_CBOR_OK);
    REQUIRE(nodes.len == 2);
    CHECK(nodes[0].node_type == TAV_CBOR_NODE_TAGGED);
    CHECK(nodes[0].value == 18);
    CHECK(nodes[nodes[0].first_child].node_type == TAV_CBOR_NODE_BYTES);
}

TEST_CASE("cbor C ABI: negative integers carry the -1-n magnitude") {
    struct Case {
        std::vector<uint8_t> input;
        uint64_t magnitude;
    };
    const std::vector<Case> cases = {
        {{0x20}, 0},                                                       // -1
        {{0x26}, 6},                                                       // -7
        {{0x3a, 0xff, 0xff, 0xff, 0xff}, 4294967295ull},                   // -(2^32)
        {{0x3b, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff},
         static_cast<uint64_t>(std::numeric_limits<int64_t>::max())},      // INT64_MIN
    };

    for (const Case& c : cases) {
        Nodes nodes;
        Buffer err;
        REQUIRE(parse_nondet(c.input, nodes, err) == TAV_CBOR_OK);
        CHECK(nodes[0].node_type == TAV_CBOR_NODE_INT);
        CHECK(nodes[0].negative == 1);
        CHECK(nodes[0].value == c.magnitude);

        Buffer out;
        Buffer serr;
        const std::vector<TavCborNode> owned = copy_of(nodes);
        REQUIRE(serialize_nondet(owned, out, serr) == TAV_CBOR_OK);
        CHECK(out.bytes() == c.input);
    }
}

TEST_CASE("cbor C ABI: a caller-built document round-trips") {
    // Assemble [1, "hi"] by hand, as a C consumer would.
    const char* text = "hi";
    std::vector<TavCborNode> nodes(3);
    nodes[0].node_type = TAV_CBOR_NODE_ARRAY;
    nodes[0].value = 2;
    nodes[0].first_child = 1;
    nodes[1].node_type = TAV_CBOR_NODE_INT;
    nodes[1].value = 1;
    nodes[2].node_type = TAV_CBOR_NODE_TEXT;
    nodes[2].ptr = reinterpret_cast<const uint8_t*>(text);
    nodes[2].len = 2;

    Buffer out;
    Buffer err;
    REQUIRE(serialize_nondet(nodes, out, err) == TAV_CBOR_OK);
    const std::vector<uint8_t> expected = {0x82, 0x01, 0x62, 0x68, 0x69};
    CHECK(out.bytes() == expected);
}

TEST_CASE("cbor C ABI: det serialization sorts map keys, nondet preserves order") {
    // {"b": 2, "a": 1} in the order given.
    const char* b = "b";
    const char* a = "a";
    std::vector<TavCborNode> nodes(5);
    nodes[0].node_type = TAV_CBOR_NODE_MAP;
    nodes[0].value = 2;
    nodes[0].first_child = 1;
    nodes[1].node_type = TAV_CBOR_NODE_TEXT;
    nodes[1].ptr = reinterpret_cast<const uint8_t*>(b);
    nodes[1].len = 1;
    nodes[2].node_type = TAV_CBOR_NODE_INT;
    nodes[2].value = 2;
    nodes[3].node_type = TAV_CBOR_NODE_TEXT;
    nodes[3].ptr = reinterpret_cast<const uint8_t*>(a);
    nodes[3].len = 1;
    nodes[4].node_type = TAV_CBOR_NODE_INT;
    nodes[4].value = 1;

    Buffer nondet;
    Buffer nondet_err;
    REQUIRE(serialize_nondet(nodes, nondet, nondet_err) == TAV_CBOR_OK);
    const std::vector<uint8_t> as_given = {0xa2, 0x61, 0x62, 0x02, 0x61, 0x61, 0x01};
    CHECK(nondet.bytes() == as_given);

    Buffer det;
    Buffer det_err;
    REQUIRE(serialize_det(nodes, det, det_err) == TAV_CBOR_OK);
    const std::vector<uint8_t> sorted = {0xa2, 0x61, 0x61, 0x01, 0x61, 0x62, 0x02};
    CHECK(det.bytes() == sorted);
}

TEST_CASE("cbor C ABI: det parsing rejects a non-preferred head width") {
    // 1 encoded in a one-byte head.
    const std::vector<uint8_t> input = {0x18, 0x01};

    Nodes lenient;
    Buffer lenient_err;
    CHECK(parse_nondet(input, lenient, lenient_err) == TAV_CBOR_OK);

    Nodes strict;
    Buffer strict_err;
    CHECK(parse_det(input, strict, strict_err) == TAV_CBOR_DECODE_FAILED);
    CHECK(strict_err.len > 0);
}

TEST_CASE("cbor C ABI: parse rejects trailing bytes") {
    const std::vector<uint8_t> input = {0x00, 0x00};
    Nodes nodes;
    Buffer err;
    CHECK(parse_nondet(input, nodes, err) == TAV_CBOR_DECODE_FAILED);
    CHECK(err.text().find("Trailing bytes") != std::string::npos);
}

TEST_CASE("cbor C ABI: max_depth is enforced on parse and serialize") {
    // [[[[1]]]]
    const std::vector<uint8_t> input = {0x81, 0x81, 0x81, 0x81, 0x01};

    Nodes shallow;
    Buffer shallow_err;
    CHECK(parse_nondet(input, shallow, shallow_err, 2) == TAV_CBOR_DECODE_FAILED);
    CHECK(shallow_err.text().find("Maximum CBOR nesting depth") != std::string::npos);

    Nodes nodes;
    Buffer err;
    REQUIRE(parse_nondet(input, nodes, err) == TAV_CBOR_OK);

    const std::vector<TavCborNode> owned = copy_of(nodes);
    Buffer out;
    Buffer out_err;
    CHECK(serialize_nondet(owned, out, out_err, 2) == TAV_CBOR_ENCODE_FAILED);
    CHECK(out_err.text().find("Maximum CBOR nesting depth") != std::string::npos);
}

TEST_CASE("cbor C ABI: serialization validates child indices") {
    // An array claiming a child the node array does not contain.
    std::vector<TavCborNode> nodes(1);
    nodes[0].node_type = TAV_CBOR_NODE_ARRAY;
    nodes[0].value = 1;
    nodes[0].first_child = 9;

    Buffer out;
    Buffer err;
    CHECK(serialize_nondet(nodes, out, err) == TAV_CBOR_ENCODE_FAILED);
    CHECK(err.text().find("out of bounds") != std::string::npos);
}

TEST_CASE("cbor C ABI: serialization rejects unknown node types") {
    std::vector<TavCborNode> nodes(1);
    nodes[0].node_type = 42;

    Buffer out;
    Buffer err;
    CHECK(serialize_nondet(nodes, out, err) == TAV_CBOR_ENCODE_FAILED);
    CHECK(err.text().find("Unknown CBOR node type") != std::string::npos);
}

TEST_CASE("cbor C ABI: NULL error out-parameters suppress the message") {
    const std::vector<uint8_t> input = {0xff};
    TavCborNode* out_ptr = nullptr;
    size_t out_len = 0;
    CHECK(tav_cbor_parse_nondet(input.data(), input.size(), kMaxDepth, &out_ptr, &out_len, nullptr,
                                nullptr) == TAV_CBOR_DECODE_FAILED);
    CHECK(out_ptr == nullptr);
    CHECK(out_len == 0);
}

TEST_CASE("cbor C ABI: output parameters are untouched on failure") {
    const std::vector<uint8_t> input = {0xff};
    TavCborNode* const sentinel = reinterpret_cast<TavCborNode*>(0x1);
    TavCborNode* out_ptr = sentinel;
    size_t out_len = 12345;
    Buffer err;
    CHECK(tav_cbor_parse_nondet(input.data(), input.size(), kMaxDepth, &out_ptr, &out_len, &err.ptr,
                                &err.len) == TAV_CBOR_DECODE_FAILED);
    CHECK(out_ptr == sentinel);
    CHECK(out_len == 12345);
    CHECK(err.len > 0);
}

TEST_CASE("cbor C ABI: freeing NULL is a no-op") {
    tav_cbor_nodes_free(nullptr, 0);
    tav_cbor_buffer_free(nullptr, 0);
    tav_cbor_nodes_free(nullptr, 7);
    tav_cbor_buffer_free(nullptr, 7);
    CHECK(true);
}

TEST_CASE("cbor C ABI: an empty document is rejected") {
    const std::vector<uint8_t> input;
    TavCborNode* out_ptr = nullptr;
    size_t out_len = 0;
    Buffer err;
    CHECK(tav_cbor_parse_nondet(input.data(), input.size(), kMaxDepth, &out_ptr, &out_len, &err.ptr,
                                &err.len) == TAV_CBOR_DECODE_FAILED);
}
