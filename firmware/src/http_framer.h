// Finds where one HTTP/1.1 response ends in a byte stream with no message boundaries, such as
// an IPP-over-USB bulk IN endpoint. Body bytes are handed to a callback as they arrive.
#ifndef HTTP_FRAMER_H
#define HTTP_FRAMER_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define HTTP_FRAMER_HEAD_MAX 2048
#define HTTP_FRAMER_LINE_MAX 64

typedef enum {
    HTTP_FRAMER_HEAD,
    HTTP_FRAMER_BODY,
    HTTP_FRAMER_CHUNK_SIZE,
    HTTP_FRAMER_CHUNK_DATA,
    HTTP_FRAMER_CHUNK_END,
    HTTP_FRAMER_TRAILER,
    HTTP_FRAMER_DONE,
    HTTP_FRAMER_ERROR,
} http_framer_state_t;

typedef void (*http_body_fn)(void *ctx, const uint8_t *data, size_t len);

typedef struct {
    http_framer_state_t state;
    int status;
    bool chunked;
    bool saw_transfer_encoding;
    uint32_t content_length;
    uint32_t remaining;
    const char *error;
    http_body_fn on_body;
    void *ctx;
    char head[HTTP_FRAMER_HEAD_MAX + 1];
    size_t head_len;
    char line[HTTP_FRAMER_LINE_MAX + 1];
    size_t line_len;
    bool line_overflow;
    size_t crlf_idx;  // position within the 2-byte CRLF expected right after chunk data
} http_framer_t;

void http_framer_init(http_framer_t *framer, http_body_fn on_body, void *ctx);

// Consumes bytes of one response and returns how many it used. It uses fewer than len only once
// the response is complete (state DONE) or malformed (state ERROR, reason in framer->error).
// Interim 1xx responses are skipped and the response that follows them is framed, except 101
// Switching Protocols, which is treated as an error (IPP-over-USB never upgrades).
size_t http_framer_feed(http_framer_t *framer, const uint8_t *data, size_t len);

#endif
