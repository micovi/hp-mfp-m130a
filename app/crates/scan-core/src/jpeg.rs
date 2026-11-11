//! JPEG header facts: a page's pixel size and colour components, read from its frame header
//! without decoding the image.

/// What the frame header (SOF) of a JPEG says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JpegInfo {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// 1 for grey, 3 for colour (YCbCr or RGB), 4 for CMYK.
    pub components: u8,
    /// Progressive rather than sequential (baseline or extended) coding.
    pub progressive: bool,
}

/// Why a JPEG cannot be used as a page.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JpegError {
    /// The data does not start with a JPEG start-of-image marker.
    #[error("the page is not a JPEG image")]
    NotJpeg,
    /// The data ends before the first scan.
    #[error("the JPEG image is truncated")]
    Truncated,
    /// The marker structure is broken.
    #[error("the JPEG image is malformed at byte {offset}: {reason}")]
    Malformed {
        /// Where the problem is.
        offset: usize,
        /// What is wrong there.
        reason: &'static str,
    },
    /// The first scan comes before any frame header.
    #[error("the JPEG image has no frame header")]
    MissingFrame,
    /// A coding or frame a PDF `DCTDecode` image cannot hold.
    #[error("the JPEG image uses {0}, which a PDF cannot embed")]
    Unsupported(String),
}

/// The frame header plus the Adobe APP14 colour transform, which decides how a PDF must read
/// CMYK samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) components: Components,
    pub(crate) progressive: bool,
    /// The transform byte of an Adobe APP14 segment, when there is one.
    pub(crate) adobe_transform: Option<u8>,
}

/// The colour components a PDF can show, by count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Components {
    Grey,
    Rgb,
    Cmyk,
}

impl Components {
    fn from_count(count: u8) -> Option<Self> {
        match count {
            1 => Some(Self::Grey),
            3 => Some(Self::Rgb),
            4 => Some(Self::Cmyk),
            _ => None,
        }
    }

    fn count(self) -> u8 {
        match self {
            Self::Grey => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
        }
    }
}

impl Header {
    pub(crate) fn info(self) -> JpegInfo {
        JpegInfo {
            width: self.width.into(),
            height: self.height.into(),
            components: self.components.count(),
            progressive: self.progressive,
        }
    }
}

/// Reads the frame header of a JPEG.
///
/// # Errors
///
/// [`JpegError`] when the data is not a JPEG, is truncated or malformed before its first scan,
/// has no frame header, or uses a coding or sample format a PDF cannot embed.
pub fn jpeg_info(jpeg: &[u8]) -> Result<JpegInfo, JpegError> {
    header(jpeg).map(Header::info)
}

const SOI: u8 = 0xD8;
const EOI: u8 = 0xD9;
const SOS: u8 = 0xDA;
const DHP: u8 = 0xDE;
const APP14: u8 = 0xEE;
const TEM: u8 = 0x01;
const RST0: u8 = 0xD0;
const RST7: u8 = 0xD7;

/// Walks the segments up to the first scan, keeping the first frame header and the first Adobe
/// APP14 segment.
pub(crate) fn header(jpeg: &[u8]) -> Result<Header, JpegError> {
    if jpeg.get(..2) != Some(&[0xFF, SOI]) {
        return Err(JpegError::NotJpeg);
    }
    let mut segments = Segments { jpeg, offset: 2 };
    let mut frame = None;
    let mut adobe_transform = None;
    while let Some(segment) = segments.next_segment()? {
        if segment.marker == APP14 {
            adobe_transform = adobe_transform.or(adobe_transform_of(segment.payload));
        } else if frame.is_none() {
            frame = read_frame(&segment)?;
        }
    }
    let frame = frame.ok_or(JpegError::MissingFrame)?;
    Ok(Header {
        adobe_transform,
        ..frame
    })
}

/// One marker segment that has a length.
struct Segment<'a> {
    marker: u8,
    /// Where its marker starts.
    offset: usize,
    payload: &'a [u8],
}

/// The marker segments of a JPEG, from just after its SOI marker.
struct Segments<'a> {
    jpeg: &'a [u8],
    offset: usize,
}

impl<'a> Segments<'a> {
    /// The next segment with a length; `None` at the first scan (SOS). Markers without a length
    /// (TEM, `RSTn`) are skipped.
    fn next_segment(&mut self) -> Result<Option<Segment<'a>>, JpegError> {
        loop {
            let offset = self.offset;
            let marker = self.marker()?;
            if [0x00, SOI, EOI].contains(&marker) {
                return Err(JpegError::Malformed {
                    offset,
                    reason: "marker outside a segment",
                });
            }
            if marker != TEM && !(RST0..=RST7).contains(&marker) {
                let payload = self.payload()?;
                return Ok((marker != SOS).then_some(Segment {
                    marker,
                    offset,
                    payload,
                }));
            }
        }
    }

    /// Reads `FF`, any `FF` fill bytes, and the marker code after them.
    fn marker(&mut self) -> Result<u8, JpegError> {
        let start = self.offset;
        if *self.jpeg.get(start).ok_or(JpegError::Truncated)? != 0xFF {
            return Err(JpegError::Malformed {
                offset: start,
                reason: "expected a marker",
            });
        }
        let mut at = start;
        while self.jpeg.get(at) == Some(&0xFF) {
            at += 1;
        }
        let marker = *self.jpeg.get(at).ok_or(JpegError::Truncated)?;
        self.offset = at + 1;
        Ok(marker)
    }

    /// Reads a segment's big-endian length (which counts itself) and returns what follows it.
    fn payload(&mut self) -> Result<&'a [u8], JpegError> {
        let start = self.offset;
        let Some(&[high, low]) = self.jpeg.get(start..start + 2) else {
            return Err(JpegError::Truncated);
        };
        let length = usize::from(u16::from_be_bytes([high, low]));
        if length < 2 {
            return Err(JpegError::Malformed {
                offset: start,
                reason: "segment length under 2",
            });
        }
        let payload = self
            .jpeg
            .get(start + 2..start + length)
            .ok_or(JpegError::Truncated)?;
        self.offset = start + length;
        Ok(payload)
    }
}

/// The transform byte of an `Adobe` APP14 payload: "Adobe", version, two flag words, transform.
fn adobe_transform_of(payload: &[u8]) -> Option<u8> {
    payload
        .strip_prefix(b"Adobe")
        .and_then(|rest| rest.get(6).copied())
}

/// The frame a SOF segment describes; `None` for any other segment.
fn read_frame(segment: &Segment<'_>) -> Result<Option<Header>, JpegError> {
    let Some(progressive) = coding(segment.marker)? else {
        return Ok(None);
    };
    let short = JpegError::Malformed {
        offset: segment.offset,
        reason: "frame header shorter than its fields",
    };
    let &[
        precision,
        height_high,
        height_low,
        width_high,
        width_low,
        count,
        ref component_specs @ ..,
    ] = segment.payload
    else {
        return Err(short);
    };
    if component_specs.len() < 3 * usize::from(count) {
        return Err(short);
    }
    let unsupported = |what: String| Err(JpegError::Unsupported(what));
    if precision != 8 {
        return unsupported(format!("{precision}-bit samples"));
    }
    let Some(components) = Components::from_count(count) else {
        return unsupported(format!("{count} colour components"));
    };
    let height = u16::from_be_bytes([height_high, height_low]);
    let width = u16::from_be_bytes([width_high, width_low]);
    if height == 0 {
        return unsupported("a height defined after the first scan (DNL)".to_owned());
    }
    if width == 0 {
        return unsupported("a width of 0 pixels".to_owned());
    }
    Ok(Some(Header {
        width,
        height,
        components,
        progressive,
        adobe_transform: None,
    }))
}

/// Whether a SOF marker's coding is progressive; `None` for markers that are not SOF.
fn coding(marker: u8) -> Result<Option<bool>, JpegError> {
    let unsupported = |what: &str| Err(JpegError::Unsupported(what.to_owned()));
    match marker {
        0xC0 | 0xC1 => Ok(Some(false)),
        0xC2 => Ok(Some(true)),
        0xC3 | 0xCB => unsupported("lossless coding"),
        0xC9 | 0xCA => unsupported("arithmetic coding"),
        0xC5..=0xC7 | 0xCD..=0xCF | DHP => unsupported("hierarchical coding"),
        _ => Ok(None),
    }
}

#[cfg(test)]
pub(crate) mod tests;
