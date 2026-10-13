"""HTTP/1.1 framing and IPP encoding for talking to a printer over an IPP-over-USB interface.

IPP-over-USB carries plain HTTP/1.1 on a pair of bulk endpoints. There is no message boundary
on the wire, so a response ends only where its Content-Length or chunked encoding says it does.
"""

import struct
from collections.abc import Callable
from dataclasses import dataclass

type IppValue = int | bool | str

IPP_STATUS_ERROR_FLOOR = 0x0100

_OP_GET_PRINTER_ATTRIBUTES = 0x000B
_TAG_OPERATION_ATTRIBUTES = 0x01
_TAG_END_OF_ATTRIBUTES = 0x03
_FIRST_VALUE_TAG = 0x10
_OUT_OF_BAND_TAGS = range(0x10, 0x20)
_TAG_INTEGER = 0x21
_TAG_BOOLEAN = 0x22
_TAG_ENUM = 0x23
_TAG_RESOLUTION = 0x32
_TAG_RANGE_OF_INTEGER = 0x33
_TAG_BEGIN_COLLECTION = 0x34
_TAG_END_COLLECTION = 0x37
_STRING_TAGS = range(0x41, 0x4A)
_TAG_KEYWORD = 0x44
_TAG_URI = 0x45
_TAG_CHARSET = 0x47
_TAG_NATURAL_LANGUAGE = 0x48
_RESOLUTION_UNIT_DPI = 3


class IncompleteResponseError(Exception):
    """The device went quiet before the HTTP response was complete."""


@dataclass(frozen=True)
class HttpResponse:
    status: int
    reason: str
    headers: dict[str, str]
    body: bytes


@dataclass(frozen=True)
class IppResponse:
    version: tuple[int, int]
    status_code: int
    request_id: int
    attributes: dict[str, list[IppValue]]

    @property
    def ok(self) -> bool:
        return self.status_code < IPP_STATUS_ERROR_FLOOR


def build_request(
    method: str, path: str, body: bytes = b"", content_type: str | None = None
) -> bytes:
    """Encodes an HTTP/1.1 request the way ipp-usb sends them to USB printers."""
    lines = [f"{method} {path} HTTP/1.1", "Host: localhost", "Connection: keep-alive"]
    if content_type is not None:
        lines.append(f"Content-Type: {content_type}")
    if body or method == "POST":
        lines.append(f"Content-Length: {len(body)}")
    head = "\r\n".join(lines) + "\r\n\r\n"
    return head.encode("ascii") + body


def read_response(read: Callable[[], bytes]) -> HttpResponse:
    """Reads one HTTP/1.1 response.

    Args:
        read: Returns the next bytes from the device, possibly empty, and raises TimeoutError
            once the device has been quiet for too long.

    Raises:
        IncompleteResponseError: The device went quiet mid-response.
        ValueError: The bytes are not an HTTP/1.1 response.
    """
    reader = _Reader(read)
    status, reason, headers = _parse_head(reader.read_until(b"\r\n\r\n"))
    if "content-length" in headers:
        body = reader.read_exact(int(headers["content-length"]))
    elif headers.get("transfer-encoding", "").lower() == "chunked":
        body = _read_chunked(reader)
    elif status < 200 or status in (204, 304):  # noqa: PLR2004 (HTTP statuses without a body)
        body = b""
    else:
        body = reader.read_until_quiet()
    return HttpResponse(status, reason, headers, body)


class _Reader:
    def __init__(self, read: Callable[[], bytes]) -> None:
        self._read = read
        self._buffer = bytearray()

    def read_until(self, delimiter: bytes) -> bytes:
        while (index := self._buffer.find(delimiter)) < 0:
            self._fill(f"{delimiter!r}")
        return self._take(index + len(delimiter))

    def read_exact(self, size: int) -> bytes:
        while len(self._buffer) < size:
            self._fill(f"{size} bytes")
        return self._take(size)

    def read_until_quiet(self) -> bytes:
        while True:
            try:
                self._buffer += self._read()
            except TimeoutError:
                return self._take(len(self._buffer))

    def _fill(self, wanted: str) -> None:
        try:
            self._buffer += self._read()
        except TimeoutError as error:
            received = bytes(self._buffer[:200])
            raise IncompleteResponseError(
                f"device went quiet while waiting for {wanted}; buffered so far: {received!r}"
            ) from error

    def _take(self, size: int) -> bytes:
        taken = bytes(self._buffer[:size])
        del self._buffer[:size]
        return taken


def _parse_head(head: bytes) -> tuple[int, str, dict[str, str]]:
    status_line, *header_lines = head.decode("iso-8859-1").split("\r\n")
    version, _, rest = status_line.partition(" ")
    code, _, reason = rest.partition(" ")
    if not version.startswith("HTTP/") or not code.isdigit():
        raise ValueError(f"not an HTTP response, status line: {status_line!r}")
    headers: dict[str, str] = {}
    for line in header_lines:
        if not line:
            continue
        name, separator, value = line.partition(":")
        if not separator:
            raise ValueError(f"malformed HTTP header line: {line!r}")
        headers[name.strip().lower()] = value.strip()
    return int(code), reason, headers


def _read_chunked(reader: _Reader) -> bytes:
    body = bytearray()
    while True:
        size_field = reader.read_until(b"\r\n").split(b";")[0].strip()
        try:
            size = int(size_field, 16)
        except ValueError as error:
            raise ValueError(f"malformed chunk size: {size_field!r}") from error
        if size == 0:
            break
        body += reader.read_exact(size)
        reader.read_exact(2)
    while reader.read_until(b"\r\n") != b"\r\n":
        pass
    return bytes(body)


def encode_get_printer_attributes(
    printer_uri: str = "ipp://localhost/ipp/print", request_id: int = 1
) -> bytes:
    """Encodes an IPP 2.0 Get-Printer-Attributes request asking for every attribute."""
    request = bytearray(struct.pack(">BBHI", 2, 0, _OP_GET_PRINTER_ATTRIBUTES, request_id))
    request.append(_TAG_OPERATION_ATTRIBUTES)
    request += _encode_attribute(_TAG_CHARSET, "attributes-charset", "utf-8")
    request += _encode_attribute(_TAG_NATURAL_LANGUAGE, "attributes-natural-language", "en")
    request += _encode_attribute(_TAG_URI, "printer-uri", printer_uri)
    request += _encode_attribute(_TAG_KEYWORD, "requested-attributes", "all")
    request.append(_TAG_END_OF_ATTRIBUTES)
    return bytes(request)


def _encode_attribute(tag: int, name: str, value: str) -> bytes:
    encoded_name = name.encode("utf-8")
    encoded_value = value.encode("utf-8")
    return (
        struct.pack(">BH", tag, len(encoded_name))
        + encoded_name
        + struct.pack(">H", len(encoded_value))
        + encoded_value
    )


def decode_ipp(data: bytes) -> IppResponse:
    """Decodes an IPP response, merging all attribute groups into one mapping.

    Collection values are summarised as "<collection>"; their members are skipped.

    Raises:
        ValueError: The data is truncated or not IPP.
    """
    if len(data) < 8:  # noqa: PLR2004 (version, status and request id)
        raise ValueError(f"IPP message too short: {len(data)} bytes")
    major, minor, status_code, request_id = struct.unpack_from(">BBHI", data, 0)
    decoder = _IppDecoder(data)
    return IppResponse((major, minor), status_code, request_id, decoder.decode(offset=8))


class _IppDecoder:
    def __init__(self, data: bytes) -> None:
        self._data = data
        self._attributes: dict[str, list[IppValue]] = {}
        self._current: str | None = None
        self._collection_depth = 0

    def decode(self, offset: int) -> dict[str, list[IppValue]]:
        while offset < len(self._data):
            tag = self._data[offset]
            offset += 1
            if tag == _TAG_END_OF_ATTRIBUTES:
                return self._attributes
            if tag < _FIRST_VALUE_TAG:
                self._current = None
                continue
            name, value, offset = self._read_entry(offset)
            self._store(tag, name, value)
        raise ValueError("IPP message ends without an end-of-attributes tag")

    def _read_entry(self, offset: int) -> tuple[str, bytes, int]:
        name_length = self._unpack_length(offset)
        name = self._data[offset + 2 : offset + 2 + name_length].decode("utf-8", "replace")
        offset += 2 + name_length
        value_length = self._unpack_length(offset)
        value = self._data[offset + 2 : offset + 2 + value_length]
        return name, value, offset + 2 + value_length

    def _unpack_length(self, offset: int) -> int:
        if offset + 2 > len(self._data):
            raise ValueError(f"IPP message truncated at byte {offset}")
        (length,) = struct.unpack_from(">H", self._data, offset)
        if offset + 2 + length > len(self._data):
            raise ValueError(f"IPP field at byte {offset} runs past the end of the message")
        return length

    def _store(self, tag: int, name: str, value: bytes) -> None:
        if self._collection_depth > 0:
            if tag == _TAG_BEGIN_COLLECTION:
                self._collection_depth += 1
            elif tag == _TAG_END_COLLECTION:
                self._collection_depth -= 1
            return
        if name:
            self._current = name
            self._attributes.setdefault(name, [])
        if self._current is None:
            raise ValueError(f"IPP value (tag 0x{tag:02x}) without an attribute name")
        if tag == _TAG_BEGIN_COLLECTION:
            self._collection_depth = 1
            self._attributes[self._current].append("<collection>")
            return
        try:
            decoded = _decode_value(tag, value)
        except struct.error as error:
            raise ValueError(
                f"IPP attribute {self._current!r}: bad value 0x{value.hex()} for tag 0x{tag:02x}"
            ) from error
        self._attributes[self._current].append(decoded)


def _decode_integer(raw: bytes) -> IppValue:
    return struct.unpack(">i", raw)[0]


def _decode_resolution(raw: bytes) -> IppValue:
    x, y, unit = struct.unpack(">iib", raw)
    return f"{x}x{y}{'dpi' if unit == _RESOLUTION_UNIT_DPI else 'dpcm'}"


def _decode_range(raw: bytes) -> IppValue:
    low, high = struct.unpack(">ii", raw)
    return f"{low}-{high}"


_BINARY_DECODERS: dict[int, Callable[[bytes], IppValue]] = {
    _TAG_INTEGER: _decode_integer,
    _TAG_ENUM: _decode_integer,
    _TAG_BOOLEAN: lambda raw: raw != b"\x00",
    _TAG_RESOLUTION: _decode_resolution,
    _TAG_RANGE_OF_INTEGER: _decode_range,
}


def _decode_value(tag: int, raw: bytes) -> IppValue:
    if decoder := _BINARY_DECODERS.get(tag):
        return decoder(raw)
    if tag in _STRING_TAGS:
        return raw.decode("utf-8", "replace")
    if tag in _OUT_OF_BAND_TAGS:
        return f"<out-of-band 0x{tag:02x}>"
    return raw.hex()
