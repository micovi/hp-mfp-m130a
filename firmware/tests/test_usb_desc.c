#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

#include "check.h"
#include "usb_desc.h"

// Reads a configuration descriptor captured from the real printer (see tests/fixtures/).
static size_t load(const char *name, uint8_t *buf, size_t cap) {
    char path[512];
    snprintf(path, sizeof path, "%s/%s", FIXTURE_DIR, name);
    FILE *file = fopen(path, "rb");
    if (file == NULL) {
        fprintf(stderr, "cannot open fixture %s\n", path);
        exit(2);
    }
    size_t len = fread(buf, 1, cap, file);
    bool truncated = !feof(file);
    fclose(file);
    if (len == 0 || truncated) {
        fprintf(stderr, "fixture %s is empty or larger than the %zu-byte buffer\n", path, cap);
        exit(2);
    }
    return len;
}

static void test_ipp_interface_at_full_speed(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-full-speed.bin", config, sizeof config);
    usb_bulk_pair_t ipp = {0};
    CHECK(usb_find_bulk_pair(config, len, (usb_interface_class_t){0xFF, 0x09, 0x01}, &ipp));
    CHECK(ipp.interface_number == 3);
    CHECK(ipp.ep_out == 0x0B && ipp.ep_in == 0x8B);
    CHECK(ipp.ep_out_size == 64 && ipp.ep_in_size == 64);
    CHECK(config[ipp.ep_out_offset + 1] == 5 && config[ipp.ep_out_offset + 2] == 0x0B);
    CHECK(config[ipp.ep_in_offset + 1] == 5 && config[ipp.ep_in_offset + 2] == 0x8B);
}

static void test_ipp_interface_at_high_speed(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-high-speed.bin", config, sizeof config);
    usb_bulk_pair_t ipp = {0};
    CHECK(usb_find_bulk_pair(config, len, (usb_interface_class_t){0xFF, 0x09, 0x01}, &ipp));
    CHECK(ipp.interface_number == 3);
    CHECK(ipp.ep_out == 0x0B && ipp.ep_in == 0x8B);
    CHECK(ipp.ep_out_size == 512 && ipp.ep_in_size == 512);
    CHECK(config[ipp.ep_out_offset + 1] == 5 && config[ipp.ep_out_offset + 2] == 0x0B);
    CHECK(config[ipp.ep_in_offset + 1] == 5 && config[ipp.ep_in_offset + 2] == 0x8B);
}

static void test_other_interfaces(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-full-speed.bin", config, sizeof config);
    usb_bulk_pair_t pair = {0};
    CHECK(usb_find_bulk_pair(config, len, (usb_interface_class_t){0x07, 0x01, 0x02}, &pair));
    CHECK(pair.interface_number == 0 && pair.ep_out == 0x01 && pair.ep_in == 0x81);
    CHECK(usb_find_bulk_pair(config, len, (usb_interface_class_t){0xFF, 0x04, 0x01}, &pair));
    CHECK(pair.interface_number == 1 && pair.ep_out == 0x07 && pair.ep_in == 0x87);
    CHECK(!usb_find_bulk_pair(config, len, (usb_interface_class_t){0x07, 0x01, 0x04}, &pair));
}

static void test_malformed_descriptors(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-full-speed.bin", config, sizeof config);
    usb_bulk_pair_t pair = {0};
    CHECK(!usb_find_bulk_pair(config, 100, (usb_interface_class_t){0xFF, 0x09, 0x01}, &pair));
    uint8_t zero_length[512];
    memcpy(zero_length, config, len);
    zero_length[9] = 0;
    CHECK(!usb_find_bulk_pair(zero_length, len, (usb_interface_class_t){0xFF, 0x09, 0x01}, &pair));
    CHECK(!usb_find_bulk_pair(config, 0, (usb_interface_class_t){0xFF, 0x09, 0x01}, &pair));
}

static void test_bounds_check_rejects_overrun_endpoint(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-full-speed.bin", config, sizeof config);
    // The IPP interface's IN endpoint descriptor starts at offset 108; claiming a bLength of
    // 0x40 bytes would run it past wTotalLength (168).
    config[108] = 0x40;
    usb_bulk_pair_t pair = {0};
    CHECK(!usb_find_bulk_pair(config, len, (usb_interface_class_t){0xFF, 0x09, 0x01}, &pair));
}

static void test_truncated_buffer_is_rejected(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-full-speed.bin", config, sizeof config);
    usb_bulk_pair_t pair = {0};
    // A buffer shorter than wTotalLength (168) is rejected before the walk starts, even though
    // 113 bytes happens to land inside the IPP IN endpoint descriptor (offset 108).
    CHECK(!usb_find_bulk_pair(config, 113, (usb_interface_class_t){0xFF, 0x09, 0x01}, &pair));
    // wTotalLength (config[2..3]) claiming more bytes than the buffer actually holds.
    uint8_t inflated[512];
    memcpy(inflated, config, len);
    inflated[2] = (uint8_t)(len + 1);
    inflated[3] = 0;
    CHECK(!usb_find_bulk_pair(inflated, len, (usb_interface_class_t){0xFF, 0x09, 0x01}, &pair));
}

static void test_bad_header_is_rejected(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-full-speed.bin", config, sizeof config);
    usb_bulk_pair_t pair = {0};
    uint8_t wrong_type[512];
    memcpy(wrong_type, config, len);
    wrong_type[1] = 0x05;  // neither configuration (2) nor other-speed-configuration (7)
    CHECK(!usb_find_bulk_pair(wrong_type, len, (usb_interface_class_t){0xFF, 0x09, 0x01}, &pair));
    CHECK(!usb_find_bulk_pair(config, 8, (usb_interface_class_t){0xFF, 0x09, 0x01}, &pair));
}

static void test_short_header_is_rejected(void) {
    // A synthetic configuration whose header claims bLength 8 (not 9). Byte 8 (the header's
    // last byte) doubles as the bLength of a bogus "interface" descriptor that would start one
    // byte early and happen to match ff/09/01 with a bulk OUT/IN pair right after it, so without
    // the header's own bLength check this would be wrongly accepted instead of rejected.
    uint8_t config[] = {8,    2, 31, 0, 1, 1, 0, 0x80, 9,     // header: bLength 8 (should be 9)
                        4,    0, 0,  2, 0xFF, 0x09, 0x01, 0,  // mis-read as an interface at [8]
                        7,    5, 0x01, 2, 64, 0, 0,
                        7,    5, 0x82, 2, 64, 0, 0};
    usb_bulk_pair_t pair = {0};
    CHECK(!usb_find_bulk_pair(config, sizeof config, (usb_interface_class_t){0xFF, 0x09, 0x01},
                              &pair));
}

static void test_short_interface_descriptor_is_rejected(void) {
    // A synthetic configuration whose only interface descriptor is one byte short of the
    // required 9 (bLength 8, not 9). Its 8 present bytes already satisfy the alt-setting and
    // class/subclass/protocol match on their own, so this must be rejected for the bad bLength,
    // not accepted because the fields that matter happen to be in bounds.
    uint8_t config[] = {9, 2, 31, 0, 1, 1, 0, 0x80, 0, 8,   4,  0,  0, 2,
                        0xFF, 0x09, 0x01, 7, 5, 0x01, 2, 64, 0, 0, 7, 5, 0x82, 2, 64, 0, 0};
    usb_bulk_pair_t pair = {0};
    CHECK(!usb_find_bulk_pair(config, sizeof config, (usb_interface_class_t){0xFF, 0x09, 0x01},
                              &pair));
}

static void test_short_endpoint_descriptor_is_rejected(void) {
    // A synthetic configuration whose bulk OUT descriptor is one byte short of the required 7
    // (bLength 4, not 7). Reading past its claimed end for wMaxPacketSize would wander into the
    // next descriptor's bytes, so this must be rejected outright instead.
    uint8_t config[] = {9, 2, 29, 0, 1, 1, 0, 0x80, 0, 9, 4,  0, 0, 2, 0xFF, 0x09,
                        0x01, 0,    4, 5, 0x01, 2, 7,    5, 0x82, 2, 64, 0,    0};
    usb_bulk_pair_t pair = {0};
    CHECK(!usb_find_bulk_pair(config, sizeof config, (usb_interface_class_t){0xFF, 0x09, 0x01},
                              &pair));
}

static void test_non_endpoint_descriptor_is_ignored(void) {
    uint8_t config[] = {9, 2, 39, 0, 1, 1, 0, 0x80, 0,
                        9, 4, 0, 0, 2, 0xFF, 0x09, 0x01, 0,
                        // A class-specific descriptor (e.g. HID) that looks like a bulk IN.
                        7, 0x21, 0x83, 2, 64, 0, 0,
                        7, 5, 0x01, 2, 64, 0, 0, 7, 5, 0x82, 2, 64, 0, 0};
    usb_bulk_pair_t pair = {0};
    CHECK(usb_find_bulk_pair(config, sizeof config, (usb_interface_class_t){0xFF, 0x09, 0x01},
                             &pair));
    CHECK(pair.ep_in == 0x82 && pair.ep_in_offset == 32);
}

static void test_zero_length_descriptor_does_not_hang(void) {
    // A synthetic configuration where the second descriptor (neither an interface nor an
    // endpoint, so only the general bLength-too-short guard can catch it) claims bLength 0.
    // Without that guard, the walk's offset would never advance past it.
    uint8_t config[] = {9, 2, 11, 0, 1, 1, 0, 0x80, 0, 0, 0x0B};
    usb_bulk_pair_t pair = {0};
    CHECK(!usb_find_bulk_pair(config, sizeof config, (usb_interface_class_t){0xFF, 0x09, 0x01},
                              &pair));
}

static void test_first_bulk_endpoints_win(void) {
    // Four bulk endpoints on one matching interface: the first OUT and first IN must be the
    // ones reported, not the last.
    uint8_t config[] = {9, 2, 46, 0, 1, 1, 0, 0x80, 0, 9, 4, 0, 0, 4, 0xFF, 0x09, 0x01, 0,
                        7, 5, 0x01, 2, 64, 0, 0, 7, 5, 0x81, 2, 64, 0, 0,
                        7, 5, 0x02, 2, 64, 0, 0, 7, 5, 0x82, 2, 64, 0, 0};
    usb_bulk_pair_t pair = {0};
    CHECK(usb_find_bulk_pair(config, sizeof config, (usb_interface_class_t){0xFF, 0x09, 0x01},
                             &pair));
    CHECK(pair.ep_out == 0x01 && pair.ep_in == 0x81);
}

static void test_bulk_type_filter(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-full-speed.bin", config, sizeof config);
    // Swap the endpoint descriptors at offsets 48 (bulk IN 0x87) and 55 (interrupt IN 0x88), so
    // the non-bulk one comes first; the bulk one must still be the one reported.
    uint8_t swapped[7];
    memcpy(swapped, config + 48, sizeof swapped);
    memcpy(config + 48, config + 55, sizeof swapped);
    memcpy(config + 55, swapped, sizeof swapped);
    usb_bulk_pair_t pair = {0};
    CHECK(usb_find_bulk_pair(config, len, (usb_interface_class_t){0xFF, 0x04, 0x01}, &pair));
    CHECK(pair.ep_in == 0x87);
}

static void test_alternate_setting_filter(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-full-speed.bin", config, sizeof config);
    // Interface 3's alternate setting (offset 92 + 3) becomes 1, so it no longer matches; the
    // ff/09/01 class is also implemented by interface 4 at its default alternate setting.
    config[95] = 1;
    usb_bulk_pair_t pair = {0};
    CHECK(usb_find_bulk_pair(config, len, (usb_interface_class_t){0xFF, 0x09, 0x01}, &pair));
    CHECK(pair.interface_number == 4);
    CHECK(pair.ep_out == 0x0C && pair.ep_in == 0x8C);
}

int main(void) {
    test_ipp_interface_at_full_speed();
    test_ipp_interface_at_high_speed();
    test_other_interfaces();
    test_malformed_descriptors();
    test_bounds_check_rejects_overrun_endpoint();
    test_truncated_buffer_is_rejected();
    test_bad_header_is_rejected();
    test_short_header_is_rejected();
    test_short_interface_descriptor_is_rejected();
    test_short_endpoint_descriptor_is_rejected();
    test_non_endpoint_descriptor_is_ignored();
    test_zero_length_descriptor_does_not_hang();
    test_first_bulk_endpoints_win();
    test_bulk_type_filter();
    test_alternate_setting_filter();
    return CHECK_RESULT();
}
