#include <string.h>

#include "check.h"
#include "http_framer.h"

typedef struct {
    char body[256];
    size_t len;
} sink_t;

static void collect(void *ctx, const uint8_t *data, size_t len) {
    sink_t *sink = ctx;
    memcpy(sink->body + sink->len, data, len);
    sink->len += len;
}

// Feeds text in pieces of `step` bytes; returns the total consumed.
static size_t feed(http_framer_t *framer, const char *text, size_t step) {
    size_t total = strlen(text);
    size_t used = 0;
    for (size_t offset = 0; offset < total; offset += step) {
        size_t piece = total - offset < step ? total - offset : step;
        size_t consumed = http_framer_feed(framer, (const uint8_t *)text + offset, piece);
        used += consumed;
        if (consumed < piece) {
            break;
        }
    }
    return used;
}

static void test_content_length_in_any_split(void) {
    const char *response = "HTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\n"
                           "Content-Length: 5\r\n\r\nhello";
    for (size_t step = 1; step <= strlen(response); step++) {
        sink_t sink = {0};
        http_framer_t framer;
        http_framer_init(&framer, collect, &sink);
        CHECK(feed(&framer, response, step) == strlen(response));
        CHECK(framer.state == HTTP_FRAMER_DONE);
        CHECK(framer.status == 200);
        CHECK(framer.content_length == 5);
        CHECK(sink.len == 5 && memcmp(sink.body, "hello", 5) == 0);
    }
}

static void test_stops_at_the_end_of_the_response(void) {
    const char *stream = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhiHTTP/1.1";
    http_framer_t framer;
    http_framer_init(&framer, NULL, NULL);
    CHECK(feed(&framer, stream, 64) == strlen(stream) - strlen("HTTP/1.1"));
    CHECK(framer.state == HTTP_FRAMER_DONE);
}

static void test_chunked_with_extension_and_trailer(void) {
    const char *response = "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, Chunked\r\n\r\n"
                           "4;name=x\r\nWiki\r\n5\r\npedia\r\n0\r\nExpires: never\r\n\r\n";
    for (size_t step = 1; step <= 7; step++) {
        sink_t sink = {0};
        http_framer_t framer;
        http_framer_init(&framer, collect, &sink);
        CHECK(feed(&framer, response, step) == strlen(response));
        CHECK(framer.state == HTTP_FRAMER_DONE);
        CHECK(sink.len == 9 && memcmp(sink.body, "Wikipedia", 9) == 0);
    }
}

static void test_interim_response_is_skipped(void) {
    sink_t sink = {0};
    http_framer_t framer;
    http_framer_init(&framer, collect, &sink);
    const char *stream =
        "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
    CHECK(feed(&framer, stream, 3) == strlen(stream));
    CHECK(framer.state == HTTP_FRAMER_DONE);
    CHECK(framer.status == 200);
    CHECK(sink.len == 2);
}

static void test_responses_without_a_body(void) {
    const char *responses[] = {
        "HTTP/1.1 204 No Content\r\n\r\n",
        "HTTP/1.1 304 Not Modified\r\n\r\n",
        "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n",
    };
    for (size_t i = 0; i < sizeof responses / sizeof responses[0]; i++) {
        http_framer_t framer;
        http_framer_init(&framer, NULL, NULL);
        CHECK(feed(&framer, responses[i], 64) == strlen(responses[i]));
        CHECK(framer.state == HTTP_FRAMER_DONE);
    }
}

static void expect_error(const char *stream, const char *error) {
    http_framer_t framer;
    http_framer_init(&framer, NULL, NULL);
    feed(&framer, stream, 64);
    CHECK(framer.state == HTTP_FRAMER_ERROR);
    CHECK(framer.error != NULL && strcmp(framer.error, error) == 0);
    if (framer.error != NULL && strcmp(framer.error, error) != 0) {
        fprintf(stderr, "  got \"%s\", want \"%s\"\n", framer.error, error);
    }
}

static void test_malformed_responses(void) {
    expect_error("SOAP/1.0 200 OK\r\n\r\n", "not an HTTP/1.x status line");
    expect_error("HTTP/1.1 2x0 OK\r\n\r\n", "not an HTTP/1.x status line");
    expect_error("HTTP/1.1 200 OK\r\nno colon\r\n\r\n", "malformed header line");
    expect_error("HTTP/1.1 200 OK\r\nContent-Length: 12a\r\n\r\n", "malformed Content-Length");
    expect_error("HTTP/1.1 200 OK\r\nContent-Length: 99999999999\r\n\r\n",
                 "malformed Content-Length");
    expect_error("HTTP/1.1 200 OK\r\n\r\n",
                 "response has neither Content-Length nor chunked encoding");
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
                 "malformed chunk size");
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n123456789\r\n",
                 "chunk too large");
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nabc\r\n",
                 "chunk data longer than its size");
    expect_error("HTTP/1.1x200 OK\r\n\r\n", "not an HTTP/1.x status line");
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"
                 "\nchunk-garbage\r\n0\r\n\r\n",
                 "malformed chunk size");
    // A bare LF is not a line terminator: it is buffered as ordinary content, so "12" plus a
    // bare LF is not the complete line "12" (which would be a valid chunk size).
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n12\nhi\r\n0\r\n\r\n",
                 "malformed chunk size");
}

// feed_chunk_end must wait for both CRLF bytes: stopping input right after the lone '\r' must
// leave the framer still in CHUNK_END, not already back in CHUNK_SIZE.
static void test_chunk_end_requires_full_crlf(void) {
    const char *prefix = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nhi\r";
    http_framer_t framer;
    http_framer_init(&framer, NULL, NULL);
    size_t used = http_framer_feed(&framer, (const uint8_t *)prefix, strlen(prefix));
    CHECK(used == strlen(prefix));
    CHECK(framer.state == HTTP_FRAMER_CHUNK_END);
}

// A stale Content-Length left over from a skipped interim response must not leak into the real
// response that follows it, even when that response carries no Content-Length of its own.
static void test_stale_content_length_is_reset(void) {
    const char *stream = "HTTP/1.1 100 Continue\r\nContent-Length: 99\r\n\r\n"
                          "HTTP/1.1 204 No Content\r\n\r\n";
    http_framer_t framer;
    http_framer_init(&framer, NULL, NULL);
    CHECK(feed(&framer, stream, 64) == strlen(stream));
    CHECK(framer.state == HTTP_FRAMER_DONE);
    CHECK(framer.content_length == 0);
}

static void test_head_too_large(void) {
    static char stream[HTTP_FRAMER_HEAD_MAX + 64];
    strcpy(stream, "HTTP/1.1 200 OK\r\nX-Filler: ");
    size_t len = strlen(stream);
    memset(stream + len, 'a', HTTP_FRAMER_HEAD_MAX);
    stream[len + HTTP_FRAMER_HEAD_MAX] = '\0';
    expect_error(stream, "response head too large");
}

static void expect_done(const char *stream, size_t len, const char *body) {
    sink_t sink = {0};
    http_framer_t framer;
    http_framer_init(&framer, collect, &sink);
    CHECK(http_framer_feed(&framer, (const uint8_t *)stream, len) == len);
    CHECK(framer.state == HTTP_FRAMER_DONE);
    CHECK(sink.len == strlen(body) && memcmp(sink.body, body, sink.len) == 0);
}

#define EXPECT_DONE(literal, body) expect_done(literal, sizeof(literal) - 1, body)

static void test_framing_edges(void) {
    EXPECT_DONE("HTTP/1.1 200\r\ncontent-length: 2\r\n\r\nhi", "hi");
    EXPECT_DONE("HTTP/1.1 200 OK\r\nContent-Length: 2 \r\n\r\nhi", "hi");
    EXPECT_DONE("HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n0\r\n\r\n", "");
    EXPECT_DONE("HTTP/1.1 200 OK\r\nContent-Length: 9\r\nTransfer-Encoding: chunked\r\n\r\n"
                "2\r\nhi\r\n0\r\n\r\n", "hi");
    EXPECT_DONE("HTTP/1.1 100 Continue\r\nTransfer-Encoding: chunked\r\n\r\n"
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi", "hi");
    EXPECT_DONE("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2 \t\r\nhi\r\n0\r\n\r\n",
                "hi");
    EXPECT_DONE("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nhi\r\n0\r\n"
                "X-Long: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                "\r\n\r\n",
                "hi");
}

static void test_length_limits(void) {
    http_framer_t framer;
    http_framer_init(&framer, NULL, NULL);
    const char *max = "HTTP/1.1 200 OK\r\nContent-Length: 4294967295\r\n\r\n";
    CHECK(feed(&framer, max, 64) == strlen(max));
    CHECK(framer.state == HTTP_FRAMER_BODY && framer.remaining == UINT32_MAX);
    http_framer_init(&framer, NULL, NULL);
    const char *chunk = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffff\r\n";
    CHECK(feed(&framer, chunk, 64) == strlen(chunk));
    CHECK(framer.state == HTTP_FRAMER_CHUNK_DATA && framer.remaining == UINT32_MAX);
}

static void test_head_at_the_limit(void) {
    static char stream[HTTP_FRAMER_HEAD_MAX + 64];
    strcpy(stream, "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nX-Filler: ");
    size_t len = strlen(stream);
    size_t filler = HTTP_FRAMER_HEAD_MAX - len - 4;
    memset(stream + len, 'a', filler);
    strcpy(stream + len + filler, "\r\n\r\n");
    expect_done(stream, strlen(stream), "");
    memmove(stream + len + 1, stream + len, filler + 5);
    expect_error(stream, "response head too large");
}

static void test_more_malformed_responses(void) {
    expect_error("HTTP/1.1 20x OK\r\n\r\n", "not an HTTP/1.x status line");
    expect_error("HTTP/1.1 200 OK\r\nContent-Length: 4294967296\r\n\r\n",
                 "malformed Content-Length");
    expect_error("HTTP/1.1 200 OK\r\nContent-Length:\r\n\r\n", "malformed Content-Length");
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n100000000\r\n",
                 "chunk too large");
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n;ext\r\n",
                 "malformed chunk size");
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nhi\n0\r\n\r\n",
                 "chunk data longer than its size");
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"
                 "000000000000000000000000000000000000000000000000000000000000000000002\r\n",
                 "malformed chunk size");
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"
                 "000000000000000000000000000000000000000000000000000000000000000000002\r\n"
                 "2\r\nhi\r\n",
                 "malformed chunk size");
}

static void test_nul_in_head_is_rejected(void) {
    static const char nul_status[] = "HTTP/1.1 200 OK\0\r\nContent-Length: 0\r\n\r\n";
    http_framer_t framer;
    http_framer_init(&framer, NULL, NULL);
    http_framer_feed(&framer, (const uint8_t *)nul_status, sizeof nul_status - 1);
    CHECK(framer.state == HTTP_FRAMER_ERROR);
    CHECK(framer.error != NULL && strcmp(framer.error, "NUL byte in response head") == 0);
}

static void test_conflicting_content_length(void) {
    expect_error("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 2\r\n\r\nhello",
                 "conflicting Content-Length");
    EXPECT_DONE("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 5\r\n\r\nhello", "hello");
}

static void test_bytes_after_chunk_data_fail_at_once(void) {
    http_framer_t framer;
    http_framer_init(&framer, NULL, NULL);
    const char *junk = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nhiXHTTP/1.1";
    feed(&framer, junk, 64);
    CHECK(framer.state == HTTP_FRAMER_ERROR);
}

static void test_status_line_tightening(void) {
    expect_error("HTTP/1.1x200 OK\r\n\r\n", "not an HTTP/1.x status line");
    expect_error("HTTP/1.a 200 OK\r\n\r\n", "not an HTTP/1.x status line");
    expect_error("HTTP/1.1 200OK\r\n\r\n", "not an HTTP/1.x status line");
    expect_error("HTTP/1.1 099 OK\r\n\r\n", "not an HTTP/1.x status line");
    expect_error("HTTP/1.1 101 Switching Protocols\r\n\r\n",
                 "unexpected 101 Switching Protocols");
}

static void test_transfer_encoding_last_token(void) {
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: x-notchunked\r\n\r\n",
                 "Transfer-Encoding without final chunked");
    EXPECT_DONE("HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\n\r\n0\r\n\r\n", "");
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked, gzip\r\n\r\n",
                 "Transfer-Encoding without final chunked");
    // Three comma-separated codings distinguish "last token" from "first token after a comma",
    // and the trailing space distinguishes trimming the token from using it untrimmed.
    EXPECT_DONE("HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, deflate, chunked \r\n\r\n0\r\n\r\n",
                "");
    // "chunk" (missing the final "ed") must not match "chunked" via a length-n comparison.
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunk\r\n\r\n",
                 "Transfer-Encoding without final chunked");
}

// RFC 9112 6.3: Transfer-Encoding overrides Content-Length, and a response whose final coding
// is not chunked would run to connection close, which USB framing cannot represent.
static void test_transfer_encoding_requires_final_chunked(void) {
    expect_error("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nTransfer-Encoding: gzip\r\n\r\nhello",
                 "Transfer-Encoding without final chunked");
}

static void test_lone_cr_or_lf_in_header_is_not_a_line_break(void) {
    expect_error("HTTP/1.1 200 OK\r\nContent-Length: 2\nX: 1\r\n\r\nhi",
                 "malformed Content-Length");
    expect_error("HTTP/1.1 200 OK\r\nContent-Length: 2\rX: 1\r\n\r\nhi",
                 "malformed Content-Length");
}

static void test_chunk_size_must_start_with_a_hex_digit(void) {
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n+2\r\nhi\r\n0\r\n\r\n",
                 "malformed chunk size");
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n 2\r\nhi\r\n0\r\n\r\n",
                 "malformed chunk size");
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0x2\r\nhi\r\n0\r\n\r\n",
                 "malformed chunk size");
    // A sign makes strtoull wrap around to a huge unsigned value; it must be rejected as
    // malformed up front, not classified as "chunk too large".
    expect_error("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n-1\r\nhi\r\n0\r\n\r\n",
                 "malformed chunk size");
}

int main(void) {
    test_content_length_in_any_split();
    test_stops_at_the_end_of_the_response();
    test_chunked_with_extension_and_trailer();
    test_interim_response_is_skipped();
    test_responses_without_a_body();
    test_malformed_responses();
    test_chunk_end_requires_full_crlf();
    test_stale_content_length_is_reset();
    test_head_too_large();
    test_framing_edges();
    test_length_limits();
    test_head_at_the_limit();
    test_more_malformed_responses();
    test_nul_in_head_is_rejected();
    test_conflicting_content_length();
    test_bytes_after_chunk_data_fail_at_once();
    test_status_line_tightening();
    test_transfer_encoding_last_token();
    test_transfer_encoding_requires_final_chunked();
    test_lone_cr_or_lf_in_header_is_not_a_line_break();
    test_chunk_size_must_start_with_a_hex_digit();
    return CHECK_RESULT();
}
