import struct
from collections.abc import Callable

import pytest

from httpipp import (
    IncompleteResponseError,
    build_request,
    decode_ipp,
    encode_get_printer_attributes,
    read_response,
)


def device(*chunks: bytes) -> Callable[[], bytes]:
    """A fake USB IN endpoint: yields the chunks, then goes quiet."""
    pending = list(chunks)

    def read() -> bytes:
        if not pending:
            raise TimeoutError("quiet")
        return pending.pop(0)

    return read


def entry(tag: int, name: str, value: bytes) -> bytes:
    encoded = name.encode()
    return struct.pack(">BH", tag, len(encoded)) + encoded + struct.pack(">H", len(value)) + value


def ipp_message(status: int, *groups: tuple[int, bytes]) -> bytes:
    body = struct.pack(">BBHI", 2, 0, status, 7)
    for group_tag, entries in groups:
        body += bytes([group_tag]) + entries
    return body + b"\x03"


class TestBuildRequest:
    def test_get_has_no_body_headers(self) -> None:
        request = build_request("GET", "/eSCL/ScannerCapabilities")
        assert request.startswith(b"GET /eSCL/ScannerCapabilities HTTP/1.1\r\n")
        assert b"Content-Length" not in request
        assert request.endswith(b"\r\n\r\n")

    def test_post_carries_length_type_and_body(self) -> None:
        request = build_request("POST", "/ipp/print", b"abc", content_type="application/ipp")
        head, body = request.split(b"\r\n\r\n")
        assert b"Content-Type: application/ipp" in head
        assert b"Content-Length: 3" in head
        assert body == b"abc"


class TestReadResponse:
    def test_content_length_body_split_across_reads_and_empty_packets(self) -> None:
        response = read_response(
            device(b"HTTP/1.1 200 OK\r\nContent-Le", b"ngth: 5\r\n\r\nhe", b"", b"llo")
        )
        assert (response.status, response.reason, response.body) == (200, "OK", b"hello")

    def test_bytes_after_the_response_are_not_consumed_into_it(self) -> None:
        response = read_response(device(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhiEXTRA"))
        assert response.body == b"hi"

    def test_chunked_body_with_extension_and_trailer(self) -> None:
        response = read_response(
            device(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
                b"4;name=x\r\nWiki\r\n5\r\npedia\r\n",
                b"0\r\nExpires: never\r\n\r\n",
            )
        )
        assert response.body == b"Wikipedia"

    def test_body_without_length_runs_until_the_device_goes_quiet(self) -> None:
        response = read_response(device(b"HTTP/1.1 200 OK\r\n\r\npart1", b"part2"))
        assert response.body == b"part1part2"

    def test_no_content_has_no_body(self) -> None:
        response = read_response(device(b"HTTP/1.1 204 No Content\r\n\r\n"))
        assert (response.status, response.body) == (204, b"")

    def test_headers_are_case_insensitive(self) -> None:
        response = read_response(device(b"HTTP/1.1 404 Not Found\r\nCONTENT-length: 0\r\n\r\n"))
        assert response.headers == {"content-length": "0"}

    def test_device_going_quiet_mid_body(self) -> None:
        with pytest.raises(IncompleteResponseError, match="5 bytes"):
            read_response(device(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhe"))

    def test_device_silent_from_the_start(self) -> None:
        with pytest.raises(IncompleteResponseError):
            read_response(device())

    def test_not_http(self) -> None:
        with pytest.raises(ValueError, match="not an HTTP response"):
            read_response(device(b"\x02\x00\x00\x0b garbage\r\n\r\n"))

    def test_malformed_header(self) -> None:
        with pytest.raises(ValueError, match="malformed HTTP header"):
            read_response(device(b"HTTP/1.1 200 OK\r\nno colon here\r\n\r\n"))

    def test_malformed_chunk_size(self) -> None:
        with pytest.raises(ValueError, match="malformed chunk size"):
            read_response(device(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n"))


class TestIpp:
    def test_get_printer_attributes_request_decodes_back(self) -> None:
        request = decode_ipp(encode_get_printer_attributes(request_id=42))
        assert request.version == (2, 0)
        assert request.status_code == 0x000B
        assert request.request_id == 42
        assert request.attributes == {
            "attributes-charset": ["utf-8"],
            "attributes-natural-language": ["en"],
            "printer-uri": ["ipp://localhost/ipp/print"],
            "requested-attributes": ["all"],
        }

    def test_value_types_and_additional_values(self) -> None:
        printer = (
            entry(0x23, "printer-state", struct.pack(">i", 3))
            + entry(0x22, "color-supported", b"\x00")
            + entry(0x21, "copies-default", struct.pack(">i", 1))
            + entry(0x32, "printer-resolution-default", struct.pack(">iib", 600, 600, 3))
            + entry(0x33, "copies-supported", struct.pack(">ii", 1, 99))
            + entry(0x49, "document-format-supported", b"image/urf")
            + entry(0x49, "", b"image/pwg-raster")
            + entry(0x13, "printer-alert", b"")
            + entry(0x30, "printer-input-tray", b"\x01\xff")
        )
        response = decode_ipp(ipp_message(0, (0x04, printer)))
        assert response.ok
        assert response.attributes == {
            "printer-state": [3],
            "color-supported": [False],
            "copies-default": [1],
            "printer-resolution-default": ["600x600dpi"],
            "copies-supported": ["1-99"],
            "document-format-supported": ["image/urf", "image/pwg-raster"],
            "printer-alert": ["<out-of-band 0x13>"],
            "printer-input-tray": ["01ff"],
        }

    def test_nested_collections_are_summarised_and_their_members_skipped(self) -> None:
        member = entry(0x4A, "", b"media-size") + entry(0x34, "", b"")
        member += entry(0x4A, "", b"x-dimension") + entry(0x21, "", struct.pack(">i", 21000))
        member += entry(0x37, "", b"")
        collection = entry(0x34, "media-col-ready", b"") + member + entry(0x37, "", b"")
        second = entry(0x34, "", b"") + entry(0x37, "", b"")
        after = entry(0x44, "sides-supported", b"one-sided")
        response = decode_ipp(ipp_message(0, (0x04, collection + second + after)))
        assert response.attributes == {
            "media-col-ready": ["<collection>", "<collection>"],
            "sides-supported": ["one-sided"],
        }

    def test_groups_are_merged(self) -> None:
        response = decode_ipp(
            ipp_message(
                0,
                (0x01, entry(0x47, "attributes-charset", b"utf-8")),
                (0x04, entry(0x41, "printer-info", b"M130a")),
            )
        )
        assert set(response.attributes) == {"attributes-charset", "printer-info"}

    def test_error_status_is_not_ok(self) -> None:
        assert not decode_ipp(ipp_message(0x0400)).ok

    @pytest.mark.parametrize(
        ("data", "message"),
        [
            (b"\x02\x00\x00", "too short"),
            (struct.pack(">BBHI", 2, 0, 0, 1) + b"\x04\x41\x00", "truncated"),
            (struct.pack(">BBHI", 2, 0, 0, 1) + b"\x04" + entry(0x41, "a", b"b")[:-1], "past"),
            (struct.pack(">BBHI", 2, 0, 0, 1) + b"\x04" + entry(0x41, "a", b"b"), "end-of-attr"),
            (ipp_message(0, (0x04, entry(0x41, "", b"x"))), "without an attribute name"),
            (ipp_message(0, (0x04, entry(0x21, "copies", b"\x01"))), "bad value"),
        ],
    )
    def test_malformed_ipp(self, data: bytes, message: str) -> None:
        with pytest.raises(ValueError, match=message):
            decode_ipp(data)
