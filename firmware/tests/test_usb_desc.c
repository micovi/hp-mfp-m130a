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
    // 113 bytes lands inside the IPP IN endpoint descriptor, which starts at offset 108.
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

static void test_short_interface_descriptor_is_rejected(void) {
    // A synthetic configuration: a matching interface with only a bulk OUT, then a malformed
    // 8-byte "interface" descriptor (too short to really be type 4), then a bulk IN. The IN
    // endpoint must not be picked up as if it still belonged to the first interface.
    uint8_t config[] = {
        9, 2, 40, 0, 1, 1, 0, 0x80, 0,       // configuration header, wTotalLength = 40
        9, 4, 0, 0, 2, 0xFF, 0x09, 0x01, 0,  // interface 0, class ff/09/01
        7, 5, 0x01, 0x02, 64, 0, 0,          // bulk OUT 0x01
        8, 4, 0, 0, 0, 0, 0, 0,              // malformed: type 4 (interface) but only 8 bytes
        7, 5, 0x82, 0x02, 64, 0, 0,          // bulk IN 0x82
    };
    usb_bulk_pair_t pair = {0};
    CHECK(!usb_find_bulk_pair(config, sizeof config, (usb_interface_class_t){0xFF, 0x09, 0x01},
                              &pair));
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
    test_short_interface_descriptor_is_rejected();
    test_bulk_type_filter();
    test_alternate_setting_filter();
    return CHECK_RESULT();
}
