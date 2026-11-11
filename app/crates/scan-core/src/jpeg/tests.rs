use super::*;
use crate::testing::fixture;

const SOI: [u8; 2] = [0xFF, 0xD8];
const EOI: [u8; 2] = [0xFF, 0xD9];

pub(crate) fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
    let length = u16::try_from(payload.len() + 2).unwrap();
    let mut bytes = vec![0xFF, marker];
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

pub(crate) fn frame(marker: u8, precision: u8, height: u16, width: u16, components: u8) -> Vec<u8> {
    let mut payload = vec![precision];
    payload.extend_from_slice(&height.to_be_bytes());
    payload.extend_from_slice(&width.to_be_bytes());
    payload.push(components);
    for id in 1..=components {
        payload.extend_from_slice(&[id, 0x11, 0]);
    }
    segment(marker, &payload)
}

pub(crate) fn sof0(height: u16, width: u16, components: u8) -> Vec<u8> {
    frame(0xC0, 8, height, width, components)
}

pub(crate) fn scan_start() -> Vec<u8> {
    segment(0xDA, &[1, 1, 0, 0, 63, 0])
}

/// SOI, the given segments, a scan with some entropy-coded bytes, EOI.
pub(crate) fn jpeg(segments: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = SOI.to_vec();
    for segment in segments {
        bytes.extend_from_slice(segment);
    }
    bytes.extend_from_slice(&scan_start());
    bytes.extend_from_slice(&[0x12, 0xFF, 0x00, 0x34]);
    bytes.extend_from_slice(&EOI);
    bytes
}

fn info(width: u32, height: u32, components: u8, progressive: bool) -> JpegInfo {
    JpegInfo {
        width,
        height,
        components,
        progressive,
    }
}

#[test]
fn reads_the_grey_fixture() {
    let jpeg = fixture("synthetic-grey-75dpi.jpg").unwrap();
    assert_eq!(jpeg_info(&jpeg), Ok(info(620, 877, 1, false)));
}

#[test]
fn reads_the_colour_fixture() {
    let jpeg = fixture("synthetic-colour-150dpi.jpg").unwrap();
    assert_eq!(jpeg_info(&jpeg), Ok(info(1240, 1754, 3, false)));
}

#[test]
fn reads_baseline_extended_and_progressive_frames() {
    assert_eq!(
        jpeg_info(&jpeg(&[frame(0xC0, 8, 2, 3, 1)])),
        Ok(info(3, 2, 1, false))
    );
    assert_eq!(
        jpeg_info(&jpeg(&[frame(0xC1, 8, 0x1234, 0xABCD, 3)])),
        Ok(info(0xABCD, 0x1234, 3, false))
    );
    assert_eq!(
        jpeg_info(&jpeg(&[frame(0xC2, 8, 65535, 65535, 4)])),
        Ok(info(65535, 65535, 4, true))
    );
}

#[test]
fn skips_other_segments_fill_bytes_and_standalone_markers() {
    let mut fill = vec![0xFF, 0xFF, 0xFF];
    fill.extend_from_slice(&segment(0xFE, b"comment"));
    let bytes = jpeg(&[
        segment(0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"),
        segment(0xDB, &[0; 65]),
        segment(0xE1, &[]),
        fill,
        vec![0xFF, 0x01],
        vec![0xFF, 0xD3],
        segment(0xC4, &[0; 29]),
        segment(0xCC, &[0; 2]),
        segment(0xC8, &[0; 2]),
        sof0(10, 20, 3),
        segment(0xDD, &[0, 4]),
    ]);
    assert_eq!(jpeg_info(&bytes), Ok(info(20, 10, 3, false)));
}

#[test]
fn the_first_frame_header_wins() {
    let bytes = jpeg(&[sof0(10, 20, 1), frame(0xC2, 8, 30, 40, 3)]);
    assert_eq!(jpeg_info(&bytes), Ok(info(20, 10, 1, false)));
}

#[test]
fn data_that_is_not_a_jpeg_is_an_error() {
    for bytes in [&b""[..], b"\xFF", b"\x89PNG\r\n", b"\xD8\xFF", b"\xFF\xD9"] {
        assert_eq!(jpeg_info(bytes), Err(JpegError::NotJpeg), "{bytes:?}");
    }
    assert_eq!(
        JpegError::NotJpeg.to_string(),
        "the page is not a JPEG image"
    );
}

#[test]
fn data_that_ends_before_the_scan_is_truncated() {
    let whole = jpeg(&[segment(0xE0, &[0; 14]), sof0(10, 20, 1)]);
    let scan_end = whole.len() - 4 - 2;
    for end in 2..scan_end {
        assert_eq!(
            jpeg_info(&whole[..end]),
            Err(JpegError::Truncated),
            "cut at {end}"
        );
    }
    assert!(jpeg_info(&whole[..scan_end]).is_ok());
    assert_eq!(
        JpegError::Truncated.to_string(),
        "the JPEG image is truncated"
    );
}

#[test]
fn a_frame_header_shorter_than_its_fields_is_malformed() {
    let app0 = segment(0xE0, &[0; 14]);
    let short = Err(JpegError::Malformed {
        offset: 2 + app0.len(),
        reason: "frame header shorter than its fields",
    });
    let three_components = [8, 0, 10, 0, 20, 3, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0];
    for end in [5, 6, 8, 14] {
        let frame = segment(0xC0, &three_components[..end]);
        assert_eq!(
            jpeg_info(&jpeg(&[app0.clone(), frame])),
            short,
            "{end} bytes"
        );
    }
    let frame = segment(0xC0, &three_components);
    assert_eq!(jpeg_info(&jpeg(&[app0, frame])), Ok(info(20, 10, 3, false)));
}

#[test]
fn markers_that_cannot_appear_between_segments_are_malformed() {
    let outside = |offset| {
        Err(JpegError::Malformed {
            offset,
            reason: "marker outside a segment",
        })
    };
    let app = segment(0xE0, &[1, 2]);
    for marker in [0x00, SOI[1], EOI[1]] {
        let mut bytes = SOI.to_vec();
        bytes.extend_from_slice(&app);
        bytes.extend_from_slice(&[0xFF, 0xFF, marker]);
        bytes.extend_from_slice(&sof0(1, 1, 1));
        bytes.extend_from_slice(&scan_start());
        assert_eq!(jpeg_info(&bytes), outside(8), "{marker:#x}");
    }
    let mut end_after_frame = SOI.to_vec();
    end_after_frame.extend_from_slice(&sof0(1, 1, 1));
    end_after_frame.extend_from_slice(&EOI);
    assert_eq!(
        jpeg_info(&end_after_frame),
        outside(2 + sof0(1, 1, 1).len())
    );
}

#[test]
fn a_scan_before_any_frame_header_is_an_error() {
    assert_eq!(jpeg_info(&jpeg(&[])), Err(JpegError::MissingFrame));
    let app = segment(0xE0, &[1, 2]);
    assert_eq!(jpeg_info(&jpeg(&[app])), Err(JpegError::MissingFrame));
    assert_eq!(
        JpegError::MissingFrame.to_string(),
        "the JPEG image has no frame header"
    );
}

#[test]
fn arithmetic_lossless_and_hierarchical_frames_are_unsupported() {
    let cases = [
        (0xC3, "lossless coding"),
        (0xC5, "hierarchical coding"),
        (0xC6, "hierarchical coding"),
        (0xC7, "hierarchical coding"),
        (0xC9, "arithmetic coding"),
        (0xCA, "arithmetic coding"),
        (0xCB, "lossless coding"),
        (0xCD, "hierarchical coding"),
        (0xCE, "hierarchical coding"),
        (0xCF, "hierarchical coding"),
        (0xDE, "hierarchical coding"),
    ];
    for (marker, what) in cases {
        let error = jpeg_info(&jpeg(&[frame(marker, 8, 10, 20, 1)])).unwrap_err();
        assert_eq!(
            error,
            JpegError::Unsupported(what.to_owned()),
            "{marker:#x}"
        );
    }
    assert_eq!(
        JpegError::Unsupported("arithmetic coding".to_owned()).to_string(),
        "the JPEG image uses arithmetic coding, which a PDF cannot embed"
    );
}

#[test]
fn frames_a_pdf_cannot_show_are_unsupported() {
    let cases = [
        (frame(0xC0, 12, 10, 20, 1), "12-bit samples"),
        (frame(0xC1, 16, 10, 20, 3), "16-bit samples"),
        (sof0(10, 20, 2), "2 colour components"),
        (sof0(10, 20, 0), "0 colour components"),
        (sof0(10, 20, 5), "5 colour components"),
        (
            sof0(0, 20, 1),
            "a height defined after the first scan (DNL)",
        ),
        (sof0(10, 0, 1), "a width of 0 pixels"),
    ];
    for (frame, what) in cases {
        assert_eq!(
            jpeg_info(&jpeg(&[frame])),
            Err(JpegError::Unsupported(what.to_owned()))
        );
    }
}

#[test]
fn bad_segment_structure_is_malformed() {
    let mut no_marker = SOI.to_vec();
    no_marker.extend_from_slice(&[0x00, 0xE0, 0, 2]);
    assert_eq!(
        jpeg_info(&no_marker),
        Err(JpegError::Malformed {
            offset: 2,
            reason: "expected a marker",
        })
    );
    let mut garbage_after_segment = SOI.to_vec();
    garbage_after_segment.extend_from_slice(&segment(0xE0, &[1, 2]));
    garbage_after_segment.extend_from_slice(&[0x42]);
    garbage_after_segment.extend_from_slice(&sof0(1, 1, 1));
    assert_eq!(
        jpeg_info(&garbage_after_segment),
        Err(JpegError::Malformed {
            offset: 8,
            reason: "expected a marker",
        })
    );
    for length in [0_u8, 1] {
        let mut bytes = SOI.to_vec();
        bytes.extend_from_slice(&[0xFF, 0xE1, 0, length, 0, 0]);
        assert_eq!(
            jpeg_info(&bytes),
            Err(JpegError::Malformed {
                offset: 4,
                reason: "segment length under 2",
            })
        );
    }
    assert_eq!(
        JpegError::Malformed {
            offset: 4,
            reason: "segment length under 2"
        }
        .to_string(),
        "the JPEG image is malformed at byte 4: segment length under 2"
    );
}

pub(crate) fn adobe(transform: u8) -> Vec<u8> {
    let mut payload = b"Adobe".to_vec();
    payload.extend_from_slice(&[0, 100, 0, 0, 0, 0, transform]);
    segment(0xEE, &payload)
}

#[test]
fn reports_the_adobe_transform() {
    let cmyk = |segments: &[Vec<u8>]| header(&jpeg(segments)).unwrap().adobe_transform;
    assert_eq!(cmyk(&[sof0(1, 1, 4)]), None);
    assert_eq!(cmyk(&[adobe(0), sof0(1, 1, 4)]), Some(0));
    assert_eq!(cmyk(&[adobe(2), sof0(1, 1, 4)]), Some(2));
    assert_eq!(cmyk(&[sof0(1, 1, 4), adobe(0)]), Some(0));

    let short = segment(0xEE, b"Adobe\0\x64\0\0\0\0");
    assert_eq!(cmyk(&[short, sof0(1, 1, 4)]), None);
    let not_adobe = segment(0xEE, b"Abode\0\x64\0\0\0\0\0");
    assert_eq!(cmyk(&[not_adobe.clone(), sof0(1, 1, 4)]), None);
    let other_app = segment(0xED, b"Adobe\0\x64\0\0\0\0\0");
    assert_eq!(cmyk(&[other_app, sof0(1, 1, 4)]), None);
    assert_eq!(cmyk(&[adobe(0), adobe(2), sof0(1, 1, 4)]), Some(0));
    assert_eq!(cmyk(&[adobe(2), not_adobe, sof0(1, 1, 4)]), Some(2));
}

#[test]
fn header_and_jpeg_info_agree() {
    let bytes = jpeg(&[adobe(0), frame(0xC2, 8, 7, 9, 4)]);
    let parsed = header(&bytes).unwrap();
    assert_eq!(parsed.info(), info(9, 7, 4, true));
    assert_eq!((parsed.width, parsed.height), (9, 7));
    assert_eq!(jpeg_info(&bytes), Ok(parsed.info()));
}
