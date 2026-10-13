#include "http_framer.h"

#include <ctype.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>

static size_t fail(http_framer_t *framer, const char *error) {
    framer->state = HTTP_FRAMER_ERROR;
    framer->error = error;
    return 0;
}

void http_framer_init(http_framer_t *framer, http_body_fn on_body, void *ctx) {
    memset(framer, 0, sizeof *framer);
    framer->state = HTTP_FRAMER_HEAD;
    framer->on_body = on_body;
    framer->ctx = ctx;
}

static bool parse_decimal(const char *text, uint32_t *value) {
    while (*text == ' ' || *text == '\t') {
        text++;
    }
    if (!isdigit((unsigned char)*text)) {
        return false;
    }
    char *end = NULL;
    unsigned long long parsed = strtoull(text, &end, 10);
    while (*end == ' ' || *end == '\t') {
        end++;
    }
    if (*end != '\0' || parsed > UINT32_MAX) {
        return false;
    }
    *value = (uint32_t)parsed;
    return true;
}

// Transfer-Encoding lists codings in the order they were applied; chunked framing applies only
// when "chunked" is the last one, trimmed of surrounding whitespace, matched case-insensitively.
// A substring match (e.g. "x-notchunked") must not count.
static bool last_coding_is_chunked(const char *value) {
    const char *comma = strrchr(value, ',');
    const char *token = comma != NULL ? comma + 1 : value;
    while (*token == ' ' || *token == '\t') {
        token++;
    }
    size_t len = strlen(token);
    while (len > 0 && (token[len - 1] == ' ' || token[len - 1] == '\t')) {
        len--;
    }
    return len == strlen("chunked") && strncasecmp(token, "chunked", len) == 0;
}

// Parses one "Name: value" header line, already split out and NUL-terminated by parse_fields.
// Returns an error message, or NULL once the field (if recognized) has been applied.
static const char *parse_field(http_framer_t *framer, char *line, bool *has_length) {
    char *colon = strchr(line, ':');
    if (colon == NULL) {
        return "malformed header line";
    }
    *colon = '\0';
    const char *value = colon + 1;
    if (strcasecmp(line, "Content-Length") == 0) {
        uint32_t length = 0;
        if (!parse_decimal(value, &length)) {
            return "malformed Content-Length";
        }
        if (*has_length && length != framer->content_length) {
            return "conflicting Content-Length";
        }
        framer->content_length = length;
        *has_length = true;
    } else if (strcasecmp(line, "Transfer-Encoding") == 0) {
        framer->saw_transfer_encoding = true;
        framer->chunked = last_coding_is_chunked(value);
    }
    return NULL;
}

// Reads the header fields after the status line; returns an error message or NULL. Lines are
// split strictly on the two-byte sequence "\r\n", unlike strtok: a lone CR or LF inside a header
// stays part of its line instead of splitting it.
static const char *parse_fields(http_framer_t *framer, char *fields, bool *has_length) {
    char *line = fields;
    for (char *eol = strstr(line, "\r\n"); eol != line; eol = strstr(line, "\r\n")) {
        *eol = '\0';
        const char *error = parse_field(framer, line, has_length);
        if (error != NULL) {
            return error;
        }
        line = eol + 2;
    }
    return NULL;
}

static void start_body(http_framer_t *framer, bool has_length) {
    int status = framer->status;
    if (status == 204 || status == 304) {
        framer->state = HTTP_FRAMER_DONE;
    } else if (framer->chunked) {
        framer->state = HTTP_FRAMER_CHUNK_SIZE;
    } else if (has_length) {
        framer->remaining = framer->content_length;
        framer->state = framer->remaining > 0 ? HTTP_FRAMER_BODY : HTTP_FRAMER_DONE;
    } else {
        fail(framer, "response has neither Content-Length nor chunked encoding");
    }
}

// Returns the status code of "HTTP/1.D NNN" followed by ' ' or '\r' (D and N are digits), or
// -1 if the line does not have that shape. The caller still rejects codes below 100.
static int parse_status_line(const char *head) {
    if (strncmp(head, "HTTP/1.", 7) != 0 || !isdigit((unsigned char)head[7]) ||
        head[8] != ' ') {
        return -1;
    }
    for (int i = 9; i < 12; i++) {
        if (!isdigit((unsigned char)head[i])) {
            return -1;
        }
    }
    if (head[12] != ' ' && head[12] != '\r') {
        return -1;
    }
    return (head[9] - '0') * 100 + (head[10] - '0') * 10 + (head[11] - '0');
}

static void parse_head(http_framer_t *framer) {
    int status = parse_status_line(framer->head);
    if (status < 100) {
        fail(framer, "not an HTTP/1.x status line");
        return;
    }
    if (status == 101) {
        fail(framer, "unexpected 101 Switching Protocols");
        return;
    }
    framer->status = status;
    char *fields = strstr(framer->head, "\r\n") + 2;
    bool has_length = false;
    framer->chunked = false;
    framer->content_length = 0;
    framer->saw_transfer_encoding = false;
    const char *error = parse_fields(framer, fields, &has_length);
    if (error != NULL) {
        fail(framer, error);
    } else if (framer->saw_transfer_encoding && !framer->chunked) {
        // RFC 9112 6.3: Transfer-Encoding overrides Content-Length, and without a final chunked
        // coding the body would run to connection close, which USB framing cannot represent.
        fail(framer, "Transfer-Encoding without final chunked");
    } else if (framer->status < 200) {
        framer->head_len = 0;
    } else {
        start_body(framer, has_length);
    }
}

static size_t feed_head(http_framer_t *framer, const uint8_t *data, size_t len) {
    size_t used = 0;
    while (used < len) {
        if (framer->head_len == HTTP_FRAMER_HEAD_MAX) {
            return used + fail(framer, "response head too large");
        }
        uint8_t byte = data[used++];
        if (byte == '\0') {
            return used + fail(framer, "NUL byte in response head");
        }
        framer->head[framer->head_len++] = (char)byte;
        if (framer->head_len >= 4 &&
            memcmp(framer->head + framer->head_len - 4, "\r\n\r\n", 4) == 0) {
            framer->head[framer->head_len] = '\0';
            parse_head(framer);
            return used;
        }
    }
    return used;
}

static size_t feed_data(http_framer_t *framer, const uint8_t *data, size_t len,
                        http_framer_state_t next) {
    size_t take = len < framer->remaining ? len : framer->remaining;
    if (framer->on_body != NULL) {
        framer->on_body(framer->ctx, data, take);
    }
    framer->remaining -= (uint32_t)take;
    if (framer->remaining == 0) {
        framer->state = next;
    }
    return take;
}

// Collects one CRLF-terminated line into framer->line; returns the bytes used and sets
// *complete once the CRLF has been seen. Characters past HTTP_FRAMER_LINE_MAX are dropped.
static size_t feed_line(http_framer_t *framer, const uint8_t *data, size_t len, bool *complete) {
    size_t used = 0;
    *complete = false;
    while (used < len) {
        char c = (char)data[used++];
        if (c == '\n' && framer->line_len > 0 && framer->line[framer->line_len - 1] == '\r') {
            framer->line[--framer->line_len] = '\0';
            *complete = true;
            return used;
        }
        if (framer->line_len < HTTP_FRAMER_LINE_MAX) {
            framer->line[framer->line_len++] = c;
        } else {
            framer->line_overflow = true;
            framer->line[framer->line_len - 1] = c;
        }
    }
    return used;
}

static void reset_line(http_framer_t *framer) {
    framer->line_len = 0;
    framer->line_overflow = false;
}

// A chunk size line is valid once its trailing whitespace is skipped: it must have at least one
// hex digit, no line overflow, and nothing left but end-of-line or a ";"-prefixed extension.
static bool chunk_size_valid(const http_framer_t *framer, const char *end, bool has_digits) {
    while (*end == ' ' || *end == '\t') {
        end++;
    }
    return !framer->line_overflow && has_digits && (*end == '\0' || *end == ';');
}

// strtoull(..., 16) is more lenient than a chunk-size grammar (1*HEXDIG): it skips leading
// whitespace, accepts a sign, and treats a "0x"/"0X" prefix specially. The first two are already
// excluded by requiring the line to start with a hex digit; the "0x" prefix needs its own check.
static bool starts_like_a_chunk_size(const char *line) {
    bool starts_with_hex_digit = isxdigit((unsigned char)line[0]);
    bool has_0x_prefix = line[0] == '0' && (line[1] == 'x' || line[1] == 'X');
    return starts_with_hex_digit && !has_0x_prefix;
}

static void parse_chunk_size(http_framer_t *framer) {
    char *end = NULL;
    unsigned long long size = strtoull(framer->line, &end, 16);
    bool has_digits = end != framer->line;
    if (!starts_like_a_chunk_size(framer->line) || !chunk_size_valid(framer, end, has_digits)) {
        fail(framer, "malformed chunk size");
    } else if (size > UINT32_MAX) {
        fail(framer, "chunk too large");
    } else if (size == 0) {
        framer->state = HTTP_FRAMER_TRAILER;
    } else {
        framer->remaining = (uint32_t)size;
        framer->state = HTTP_FRAMER_CHUNK_DATA;
    }
    reset_line(framer);
}

static size_t feed_chunk_line(http_framer_t *framer, const uint8_t *data, size_t len) {
    bool complete = false;
    size_t used = feed_line(framer, data, len, &complete);
    if (!complete) {
        return used;
    }
    if (framer->state == HTTP_FRAMER_CHUNK_SIZE) {
        parse_chunk_size(framer);
    } else if (framer->line_len == 0) {
        framer->state = HTTP_FRAMER_DONE;
    }
    reset_line(framer);
    return used;
}

// Only the CRLF that closes a chunk's data may follow it; any other byte fails right away, on
// that byte, rather than waiting for a full line. A dedicated index tracks position within the
// expected two-byte CRLF, since chunk data is not itself a line and must not share framer->line.
static size_t feed_chunk_end(http_framer_t *framer, const uint8_t *data, size_t len) {
    (void)len;
    static const char CRLF[2] = {'\r', '\n'};
    if (data[0] != (uint8_t)CRLF[framer->crlf_idx]) {
        return fail(framer, "chunk data longer than its size");
    }
    if (++framer->crlf_idx == 2) {
        framer->crlf_idx = 0;
        framer->state = HTTP_FRAMER_CHUNK_SIZE;
    }
    return 1;
}

static size_t feed_body(http_framer_t *framer, const uint8_t *data, size_t len) {
    return feed_data(framer, data, len, HTTP_FRAMER_DONE);
}

static size_t feed_chunk_data(http_framer_t *framer, const uint8_t *data, size_t len) {
    return feed_data(framer, data, len, HTTP_FRAMER_CHUNK_END);
}

typedef size_t (*feed_fn)(http_framer_t *framer, const uint8_t *data, size_t len);

static const feed_fn FEEDERS[] = {
    [HTTP_FRAMER_HEAD] = feed_head,
    [HTTP_FRAMER_BODY] = feed_body,
    [HTTP_FRAMER_CHUNK_SIZE] = feed_chunk_line,
    [HTTP_FRAMER_CHUNK_DATA] = feed_chunk_data,
    [HTTP_FRAMER_CHUNK_END] = feed_chunk_end,
    [HTTP_FRAMER_TRAILER] = feed_chunk_line,
};

_Static_assert(sizeof FEEDERS / sizeof FEEDERS[0] == HTTP_FRAMER_DONE,
               "a feeder per active state");

size_t http_framer_feed(http_framer_t *framer, const uint8_t *data, size_t len) {
    size_t used = 0;
    while (used < len && framer->state != HTTP_FRAMER_DONE && framer->state != HTTP_FRAMER_ERROR) {
        used += FEEDERS[framer->state](framer, data + used, len - used);
    }
    return used;
}
