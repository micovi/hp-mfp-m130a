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
    fclose(file);
    return len;
}

static void test_ipp_interface_at_full_speed(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-full-speed.bin", config, sizeof config);
    usb_bulk_pair_t ipp;
    CHECK(usb_find_bulk_pair(config, len, 0xFF, 0x09, 0x01, &ipp));
    CHECK(ipp.interface_number == 3);
    CHECK(ipp.ep_out == 0x0B && ipp.ep_in == 0x8B);
    CHECK(ipp.ep_out_size == 64 && ipp.ep_in_size == 64);
    CHECK(config[ipp.ep_out_offset + 1] == 5 && config[ipp.ep_out_offset + 2] == 0x0B);
    CHECK(config[ipp.ep_in_offset + 1] == 5 && config[ipp.ep_in_offset + 2] == 0x8B);
}

static void test_ipp_interface_at_high_speed(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-high-speed.bin", config, sizeof config);
    usb_bulk_pair_t ipp;
    CHECK(usb_find_bulk_pair(config, len, 0xFF, 0x09, 0x01, &ipp));
    CHECK(ipp.interface_number == 3);
    CHECK(ipp.ep_out_size == 512 && ipp.ep_in_size == 512);
}

static void test_other_interfaces(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-full-speed.bin", config, sizeof config);
    usb_bulk_pair_t pair;
    CHECK(usb_find_bulk_pair(config, len, 0x07, 0x01, 0x02, &pair));
    CHECK(pair.interface_number == 0 && pair.ep_out == 0x01 && pair.ep_in == 0x81);
    CHECK(usb_find_bulk_pair(config, len, 0xFF, 0x04, 0x01, &pair));
    CHECK(pair.interface_number == 1 && pair.ep_out == 0x07 && pair.ep_in == 0x87);
    CHECK(!usb_find_bulk_pair(config, len, 0x07, 0x01, 0x04, &pair));
}

static void test_malformed_descriptors(void) {
    uint8_t config[512];
    size_t len = load("m130a-config-full-speed.bin", config, sizeof config);
    usb_bulk_pair_t pair;
    CHECK(!usb_find_bulk_pair(config, 100, 0xFF, 0x09, 0x01, &pair));
    uint8_t zero_length[512];
    memcpy(zero_length, config, len);
    zero_length[9] = 0;
    CHECK(!usb_find_bulk_pair(zero_length, len, 0xFF, 0x09, 0x01, &pair));
    CHECK(!usb_find_bulk_pair(config, 0, 0xFF, 0x09, 0x01, &pair));
}

int main(void) {
    test_ipp_interface_at_full_speed();
    test_ipp_interface_at_high_speed();
    test_other_interfaces();
    test_malformed_descriptors();
    return CHECK_RESULT();
}
