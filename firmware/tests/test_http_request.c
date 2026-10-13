#include <string.h>

#include "check.h"
#include "http_request.h"

static void test_get_head(void) {
    char buf[256];
    http_head_t head = {.method = "GET", .path = "/eSCL/ScannerCapabilities"};
    size_t len = http_request_head(buf, sizeof buf, &head);
    const char *want = "GET /eSCL/ScannerCapabilities HTTP/1.1\r\nHost: localhost\r\n"
                       "Connection: keep-alive\r\n\r\n";
    CHECK(len == strlen(want) && strcmp(buf, want) == 0);
}

static void test_post_head(void) {
    char buf[256];
    http_head_t head = {
        .method = "POST", .path = "/ipp/print", .content_type = "application/ipp", .body_len = 141};
    size_t len = http_request_head(buf, sizeof buf, &head);
    const char *want = "POST /ipp/print HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n"
                       "Content-Type: application/ipp\r\nContent-Length: 141\r\n\r\n";
    CHECK(len == strlen(want) && strcmp(buf, want) == 0);
}

static void test_head_that_does_not_fit(void) {
    char buf[16];
    http_head_t head = {.method = "GET", .path = "/eSCL/ScannerCapabilities"};
    CHECK(http_request_head(buf, sizeof buf, &head) == 0);
}

// Pins the exact boundary: snprintf's returned length excludes the NUL, so a cap equal to the
// head's length leaves no room for it and must be rejected; cap + 1 is the smallest that fits.
static void test_head_needs_room_for_its_nul(void) {
    char buf[256];
    http_head_t head = {.method = "GET", .path = "/eSCL/ScannerCapabilities"};
    const char *want = "GET /eSCL/ScannerCapabilities HTTP/1.1\r\nHost: localhost\r\n"
                       "Connection: keep-alive\r\n\r\n";
    size_t exact_len = strlen(want);
    CHECK(http_request_head(buf, exact_len, &head) == 0);
    size_t len = http_request_head(buf, exact_len + 1, &head);
    CHECK(len == exact_len && strcmp(buf, want) == 0);
}

static void test_body_without_content_type_is_rejected(void) {
    char buf[256];
    http_head_t head = {.method = "POST", .path = "/ipp/print", .body_len = 141};
    CHECK(http_request_head(buf, sizeof buf, &head) == 0);
}

// The same bytes as probe/httpipp.py's encode_get_printer_attributes(request_id=1), which the
// printer answered over USB.
static const uint8_t GOLDEN_IPP[] = {
    0x02, 0x00, 0x00, 0x0b, 0x00, 0x00, 0x00, 0x01, 0x01, 0x47, 0x00, 0x12, 0x61, 0x74, 0x74,
    0x72, 0x69, 0x62, 0x75, 0x74, 0x65, 0x73, 0x2d, 0x63, 0x68, 0x61, 0x72, 0x73, 0x65, 0x74,
    0x00, 0x05, 0x75, 0x74, 0x66, 0x2d, 0x38, 0x48, 0x00, 0x1b, 0x61, 0x74, 0x74, 0x72, 0x69,
    0x62, 0x75, 0x74, 0x65, 0x73, 0x2d, 0x6e, 0x61, 0x74, 0x75, 0x72, 0x61, 0x6c, 0x2d, 0x6c,
    0x61, 0x6e, 0x67, 0x75, 0x61, 0x67, 0x65, 0x00, 0x02, 0x65, 0x6e, 0x45, 0x00, 0x0b, 0x70,
    0x72, 0x69, 0x6e, 0x74, 0x65, 0x72, 0x2d, 0x75, 0x72, 0x69, 0x00, 0x19, 0x69, 0x70, 0x70,
    0x3a, 0x2f, 0x2f, 0x6c, 0x6f, 0x63, 0x61, 0x6c, 0x68, 0x6f, 0x73, 0x74, 0x2f, 0x69, 0x70,
    0x70, 0x2f, 0x70, 0x72, 0x69, 0x6e, 0x74, 0x44, 0x00, 0x14, 0x72, 0x65, 0x71, 0x75, 0x65,
    0x73, 0x74, 0x65, 0x64, 0x2d, 0x61, 0x74, 0x74, 0x72, 0x69, 0x62, 0x75, 0x74, 0x65, 0x73,
    0x00, 0x03, 0x61, 0x6c, 0x6c, 0x03,
};

static void test_get_printer_attributes_matches_the_probe(void) {
    uint8_t buf[256];
    size_t len = ipp_get_printer_attributes(buf, sizeof buf, 1);
    CHECK(len == sizeof GOLDEN_IPP);
    CHECK(memcmp(buf, GOLDEN_IPP, sizeof GOLDEN_IPP) == 0);
}

static void test_request_id_is_big_endian(void) {
    uint8_t buf[256];
    ipp_get_printer_attributes(buf, sizeof buf, 0x01020304);
    CHECK(buf[4] == 0x01 && buf[5] == 0x02 && buf[6] == 0x03 && buf[7] == 0x04);
}

static void test_ipp_that_does_not_fit(void) {
    uint8_t buf[100];
    CHECK(ipp_get_printer_attributes(buf, sizeof buf, 1) == 0);
}

static void test_ipp_fits_exactly(void) {
    uint8_t buf[sizeof GOLDEN_IPP];
    CHECK(ipp_get_printer_attributes(buf, sizeof buf, 1) == sizeof GOLDEN_IPP);
    CHECK(ipp_get_printer_attributes(buf, sizeof buf - 1, 1) == 0);
}

int main(void) {
    test_get_head();
    test_post_head();
    test_head_that_does_not_fit();
    test_head_needs_room_for_its_nul();
    test_body_without_content_type_is_rejected();
    test_get_printer_attributes_matches_the_probe();
    test_request_id_is_big_endian();
    test_ipp_that_does_not_fit();
    test_ipp_fits_exactly();
    return CHECK_RESULT();
}
