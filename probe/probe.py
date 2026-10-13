"""Checks whether an HP LaserJet MFP M129-M134 answers IPP and eSCL on its USB interfaces.

Read-only: it sends one IPP Get-Printer-Attributes request and a few HTTP GETs, never a print or
scan job. Everything the printer answers is saved under probe-output/ for later analysis.
"""

import json
import sys
import time
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from types import TracebackType
from typing import Self
from xml.etree import ElementTree

import usb.backend.libusb1
import usb.core
import usb.util

from httpipp import (
    HttpResponse,
    IncompleteResponseError,
    build_request,
    decode_ipp,
    encode_get_printer_attributes,
    read_response,
)

HP_VENDOR_ID = 0x03F0
M129_M134_PRODUCT_ID = 0x622A
OUTPUT_DIR = Path("probe-output")
READ_SIZE = 16 * 1024
READ_TIMEOUT_MS = 5000
DRAIN_TIMEOUT_MS = 200
IDLE_POLL_S = 0.001
HTTP_OK = 200

INTERFACE_KINDS = {
    (0x07, 0x01, 0x01): "printer, unidirectional",
    (0x07, 0x01, 0x02): "printer, bidirectional",
    (0x07, 0x01, 0x04): "IPP-over-USB (standard)",
    (0xFF, 0x09, 0x01): "IPP-over-USB (HP)",
    (0xFF, 0x04, 0x01): "HP LEDM (web services)",
    (0xFF, 0x02, 0x01): "HP SOAP scan",
}
IPP_KINDS = {"IPP-over-USB (standard)", "IPP-over-USB (HP)"}
SPEEDS = {1: "low (1.5 Mbps)", 2: "full (12 Mbps)", 3: "high (480 Mbps)", 4: "super (5 Gbps)"}
IPP_KEY_ATTRIBUTES = (
    "printer-make-and-model",
    "printer-state",
    "printer-state-reasons",
    "ipp-versions-supported",
    "document-format-supported",
    "urf-supported",
    "pwg-raster-document-resolution-supported",
    "pwg-raster-document-type-supported",
    "printer-resolution-supported",
    "media-ready",
    "printer-uuid",
)
ESCL_KEY_ELEMENTS = ("MakeAndModel", "ColorMode", "XResolution", "DocumentFormatExt")
RASTER_FORMATS = {"image/urf", "image/pwg-raster"}


@dataclass(frozen=True)
class Interface:
    number: int
    alternate: int
    triple: tuple[int, int, int]
    bulk_out: int | None
    bulk_in: int | None
    out_packet_size: int

    @property
    def kind(self) -> str:
        return INTERFACE_KINDS.get(self.triple, "other")

    def describe(self) -> str:
        triple = "/".join(f"{part:02x}" for part in self.triple)
        endpoints = f"out={_hex(self.bulk_out)} in={_hex(self.bulk_in)}"
        return f"IF{self.number} alt{self.alternate}  {triple}  {endpoints}  {self.kind}"


def _hex(address: int | None) -> str:
    return "-" if address is None else f"0x{address:02x}"


class UsbStepError(Exception):
    """A USB transfer failed; the message names the interface, endpoint and step."""


class UsbHttpChannel:
    """HTTP/1.1 over the bulk endpoints of one claimed USB interface."""

    def __init__(self, device: usb.core.Device, interface: Interface) -> None:
        if interface.bulk_out is None or interface.bulk_in is None:
            raise ValueError(f"IF{interface.number} has no bulk endpoint pair")
        self._device = device
        self._interface = interface
        self._out = interface.bulk_out
        self._in = interface.bulk_in

    def __enter__(self) -> Self:
        number = self._interface.number
        try:
            usb.util.claim_interface(self._device, number)
        except usb.core.USBError as error:
            raise RuntimeError(
                f"cannot claim IF{number}: {error}. Quit HP Smart / HP Easy Scan and any other "
                "app using the printer, or unplug and replug it, then retry."
            ) from error
        if self._interface.alternate != 0:
            self._device.set_interface_altsetting(number, self._interface.alternate)
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        usb.util.release_interface(self._device, self._interface.number)

    def request(self, raw_request: bytes) -> HttpResponse:
        with self._step(f"draining stale data from IN 0x{self._in:02x}"):
            stale = self._drain()
        if stale:
            print(f"    (discarded {stale} stale bytes left in IF{self._interface.number})")
        with self._step(f"writing the request to OUT 0x{self._out:02x}"):
            self._device.write(self._out, raw_request, READ_TIMEOUT_MS)
            if len(raw_request) % self._interface.out_packet_size == 0:
                self._device.write(self._out, b"", READ_TIMEOUT_MS)
        with self._step(f"reading the response from IN 0x{self._in:02x}"):
            return read_response(self._read)

    @contextmanager
    def _step(self, what: str) -> Iterator[None]:
        try:
            yield
        except usb.core.USBError as error:
            raise UsbStepError(f"IF{self._interface.number}: {what} failed: {error}") from error

    def _read(self) -> bytes:
        """Returns the next bytes, or raises TimeoutError after READ_TIMEOUT_MS without any.

        When idle, HP's IPP interfaces answer every read at once with a zero-length packet
        instead of letting it time out, so quiet is measured in wall-clock time.
        """
        deadline = time.monotonic() + READ_TIMEOUT_MS / 1000
        while time.monotonic() < deadline:
            try:
                data = bytes(self._device.read(self._in, READ_SIZE, READ_TIMEOUT_MS))
            except usb.core.USBTimeoutError:
                break
            if data:
                return data
            time.sleep(IDLE_POLL_S)
        raise TimeoutError(f"no data from IF{self._interface.number} for {READ_TIMEOUT_MS} ms")

    def _drain(self) -> int:
        """Discards bytes left over from an earlier exchange, up to the first empty read."""
        discarded = 0
        while True:
            try:
                data = self._device.read(self._in, READ_SIZE, DRAIN_TIMEOUT_MS)
            except usb.core.USBTimeoutError:
                return discarded
            if not data:
                return discarded
            discarded += len(data)


def find_printer(backend: usb.backend.IBackend) -> usb.core.Device | None:
    """Returns the M129-M134 if connected, else any HP device, else None."""
    devices = list(usb.core.find(find_all=True, idVendor=HP_VENDOR_ID, backend=backend))
    for device in devices:
        if device.idProduct == M129_M134_PRODUCT_ID:
            return device
    return devices[0] if devices else None


def list_interfaces(device: usb.core.Device) -> list[Interface]:
    try:
        configuration = device.get_active_configuration()
    except usb.core.USBError:
        device.set_configuration()
        configuration = device.get_active_configuration()
    interfaces = []
    for setting in configuration:
        bulk = [
            endpoint
            for endpoint in setting
            if usb.util.endpoint_type(endpoint.bmAttributes) == usb.util.ENDPOINT_TYPE_BULK
        ]
        outs = [e for e in bulk if usb.util.endpoint_direction(e.bEndpointAddress) == 0]
        ins = [e for e in bulk if usb.util.endpoint_direction(e.bEndpointAddress) != 0]
        interfaces.append(
            Interface(
                number=setting.bInterfaceNumber,
                alternate=setting.bAlternateSetting,
                triple=(
                    setting.bInterfaceClass,
                    setting.bInterfaceSubClass,
                    setting.bInterfaceProtocol,
                ),
                bulk_out=outs[0].bEndpointAddress if outs else None,
                bulk_in=ins[0].bEndpointAddress if ins else None,
                out_packet_size=outs[0].wMaxPacketSize if outs else 64,
            )
        )
    return interfaces


@dataclass(frozen=True)
class Result:
    ok: bool
    detail: str

    def __str__(self) -> str:
        return f"{'OK' if self.ok else 'FAIL'}: {self.detail}"


type Probe = Callable[[UsbHttpChannel], Result]


def probe_ipp(channel: UsbHttpChannel) -> Result:
    request = build_request(
        "POST", "/ipp/print", encode_get_printer_attributes(), content_type="application/ipp"
    )
    response = channel.request(request)
    (OUTPUT_DIR / "ipp-get-printer-attributes.bin").write_bytes(response.body)
    if response.status != HTTP_OK:
        return Result(ok=False, detail=f"HTTP {response.status} {response.reason}")
    ipp = decode_ipp(response.body)
    (OUTPUT_DIR / "ipp-attributes.json").write_text(json.dumps(ipp.attributes, indent=2))
    for name in IPP_KEY_ATTRIBUTES:
        if name in ipp.attributes:
            print(f"    {name}: {', '.join(map(str, ipp.attributes[name]))}")
    if not ipp.ok:
        return Result(ok=False, detail=f"IPP status 0x{ipp.status_code:04x}")
    formats = {str(value) for value in ipp.attributes.get("document-format-supported", [])}
    raster = sorted(formats & RASTER_FORMATS)
    version = f"{ipp.version[0]}.{ipp.version[1]}"
    return Result(ok=True, detail=f"IPP {version}, raster formats: {raster or 'none'}")


def probe_escl(channel: UsbHttpChannel) -> Result:
    response = channel.request(build_request("GET", "/eSCL/ScannerCapabilities"))
    (OUTPUT_DIR / "escl-scanner-capabilities.xml").write_bytes(response.body)
    if response.status != HTTP_OK:
        return Result(ok=False, detail=f"HTTP {response.status} {response.reason}")
    found = _xml_values(response.body, ESCL_KEY_ELEMENTS)
    for name, values in found.items():
        print(f"    {name}: {', '.join(values)}")
    sources = _xml_values(response.body, ("Platen", "Adf")).keys()
    return Result(ok=True, detail=f"scanner sources: {sorted(sources) or 'unknown'}")


def probe_ledm(channel: UsbHttpChannel) -> Result:
    response = channel.request(build_request("GET", "/DevMgmt/DiscoveryTree.xml"))
    (OUTPUT_DIR / "ledm-discovery-tree.xml").write_bytes(response.body)
    if response.status != HTTP_OK:
        return Result(ok=False, detail=f"HTTP {response.status} {response.reason}")
    uris = _xml_values(response.body, ("ResourceURI",)).get("ResourceURI", [])
    for uri in uris:
        print(f"    {uri}")
    scan_uris = [uri for uri in uris if "scan" in uri.lower()]
    return Result(ok=True, detail=f"{len(uris)} resources, scan-related: {scan_uris or 'none'}")


def _xml_values(document: bytes, names: tuple[str, ...]) -> dict[str, list[str]]:
    """Collects the text of every element whose local name is in names, ignoring namespaces."""
    found: dict[str, list[str]] = {}
    for element in ElementTree.fromstring(document).iter():
        local_name = element.tag.rsplit("}", 1)[-1]
        if local_name in names:
            values = found.setdefault(local_name, [])
            text = (element.text or "").strip()
            if text and text not in values:
                values.append(text)
    return found


def run_probe(name: str, probe: Probe, channel: UsbHttpChannel) -> Result:
    """Runs one probe, turning a failure into a FAIL line so the remaining probes still run."""
    print(f"\n[{name}]")
    try:
        result = probe(channel)
    except (UsbStepError, IncompleteResponseError, ValueError, ElementTree.ParseError) as e:
        result = Result(ok=False, detail=f"{type(e).__name__}: {e}")
    print(f"  -> {result}")
    return result


def probe_interfaces(device: usb.core.Device, interfaces: list[Interface]) -> dict[str, Result]:
    results: dict[str, Result] = {}
    ipp = next((i for i in interfaces if i.kind in IPP_KINDS and i.bulk_in), None)
    if ipp is None:
        results["IPP"] = results["eSCL"] = Result(ok=False, detail="no IPP-over-USB interface")
    else:
        with UsbHttpChannel(device, ipp) as channel:
            results["IPP"] = run_probe(f"IPP on IF{ipp.number}", probe_ipp, channel)
            results["eSCL"] = run_probe(f"eSCL on IF{ipp.number}", probe_escl, channel)
    ledm = next((i for i in interfaces if i.kind == "HP LEDM (web services)" and i.bulk_in), None)
    if ledm is None:
        results["LEDM"] = Result(ok=False, detail="no LEDM interface")
    else:
        with UsbHttpChannel(device, ledm) as channel:
            results["LEDM"] = run_probe(f"LEDM on IF{ledm.number}", probe_ledm, channel)
    return results


def verdict(results: dict[str, Result]) -> str:
    if results["IPP"].ok and results["eSCL"].ok:
        return "Plan A: the Pico proxies IPP-over-USB -> AirPrint and AirScan, no Mac driver."
    if results["IPP"].ok:
        return "Plan A for printing (AirPrint); scanning needs another route (see LEDM above)."
    return "Plan B: port 9100 bridge, with the laptop rendering PCLm. No scanning this way."


def print_device(device: usb.core.Device) -> None:
    # pyusb sets the descriptor fields (idVendor, iProduct, ...) on Device at runtime.
    vendor, product_id, product_index = (
        getattr(device, field) for field in ("idVendor", "idProduct", "iProduct")
    )
    try:
        product = usb.util.get_string(device, product_index)
    except (usb.core.USBError, ValueError):
        product = "?"
    speed = SPEEDS.get(device.speed, f"unknown ({device.speed})")
    print(f"{vendor:04x}:{product_id:04x}  {product}  speed: {speed}")


def main() -> int:
    backend = usb.backend.libusb1.get_backend()
    if backend is None:
        print("libusb not found. Run inside the Nix dev shell: `nix develop`, or direnv.")
        return 2
    device = find_printer(backend)
    if device is None:
        print("No HP device on USB. Connected devices:")
        for other in usb.core.find(find_all=True, backend=backend):
            print(f"  {other.idVendor:04x}:{other.idProduct:04x}")
        return 1
    OUTPUT_DIR.mkdir(exist_ok=True)
    print_device(device)
    interfaces = list_interfaces(device)
    listing = "\n".join(interface.describe() for interface in interfaces)
    print(listing)
    (OUTPUT_DIR / "interfaces.txt").write_text(listing + "\n")
    try:
        results = probe_interfaces(device, interfaces)
    except RuntimeError as error:
        print(f"\n{error}")
        return 3
    print("\nSummary")
    for name, result in results.items():
        print(f"  {name:5} {result}")
    print(f"\n{verdict(results)}\nResponses saved in {OUTPUT_DIR}/")
    return 0


if __name__ == "__main__":
    sys.exit(main())
