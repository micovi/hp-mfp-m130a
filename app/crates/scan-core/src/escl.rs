//! eSCL documents: the scan settings sent with a job, the capabilities and status the printer
//! reports, and the job location it answers with.
//!
//! Parsing matches elements by local name: the M130a uses the `scan:` and `pwg:` prefixes, but
//! another firmware could bind the same namespaces to other prefixes or to a default namespace.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use quick_xml::Reader;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesRef, Event};

/// Grey or colour, as the app offers them; eSCL calls them `Grayscale8` and `RGB24`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorMode {
    /// 8-bit greyscale (`Grayscale8`).
    Grey,
    /// 24-bit RGB (`RGB24`).
    Colour,
}

impl ColorMode {
    fn escl_name(self) -> &'static str {
        match self {
            Self::Grey => "Grayscale8",
            Self::Colour => "RGB24",
        }
    }

    fn from_escl_name(name: &str) -> Option<Self> {
        [Self::Grey, Self::Colour]
            .into_iter()
            .find(|mode| mode.escl_name() == name)
    }
}

impl fmt::Display for ColorMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Grey => "grey",
            Self::Colour => "colour",
        })
    }
}

/// What one scan asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanSettings {
    /// Grey or colour.
    pub color: ColorMode,
    /// Resolution in dots per inch, one of [`RESOLUTIONS`] that the scanner also reports.
    pub dpi: u16,
}

/// The resolutions the app offers: 75 dpi only for quick tests, 150–600 dpi in the UI.
pub const RESOLUTIONS: [u16; 4] = [75, 150, 300, 600];

/// The A4 scan region, in the 1/300 inch units eSCL uses.
const A4_WIDTH: u32 = 2480;
const A4_HEIGHT: u32 = 3508;

const SCAN_SETTINGS_START: &str = concat!(
    r#"<scan:ScanSettings xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03""#,
    r#" xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm">"#
);

/// What the scanner reports it can do on its flatbed (platen).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    /// Such as `HP LaserJet MFP M130a`.
    pub make_and_model: String,
    /// Largest scan width, in 1/300 inch.
    pub max_width: u32,
    /// Largest scan height, in 1/300 inch.
    pub max_height: u32,
    /// Resolutions with equal X and Y, ascending.
    pub resolutions: Vec<u16>,
    /// The colour modes the app knows, in the order the scanner lists them.
    pub color_modes: Vec<ColorMode>,
}

/// One job in the scanner's status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobInfo {
    /// The job's path, such as `/eSCL/ScanJobs/1002`.
    pub uri: String,
    /// `Pending`, `Processing`, `Completed`, `Canceled`, `Aborted`, …
    pub state: String,
    /// Pages scanned so far.
    pub images_completed: u32,
}

/// The scanner's state and its recent jobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannerStatus {
    /// Whether it can take a job now.
    pub state: ScannerState,
    /// Recent jobs, as the scanner lists them (the M130a lists the newest first).
    pub jobs: Vec<JobInfo>,
}

/// The PWG scanner states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScannerState {
    /// Ready for a job.
    Idle,
    /// Busy with a job, or still starting up.
    Processing,
    /// Running a self-test.
    Testing,
    /// Stopped and needs attention.
    Stopped,
    /// Out of order.
    Down,
    /// Any state this client does not know, as sent.
    Other(String),
}

impl ScannerState {
    fn from_pwg(state: String) -> Self {
        match state.as_str() {
            "Idle" => Self::Idle,
            "Processing" => Self::Processing,
            "Testing" => Self::Testing,
            "Stopped" => Self::Stopped,
            "Down" => Self::Down,
            _ => Self::Other(state),
        }
    }
}

/// Why an eSCL document could not be built or understood.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EsclError {
    /// The scan settings ask for a resolution that the scanner or the app does not offer.
    #[error("{}", resolution_message(*dpi, supported))]
    UnsupportedResolution {
        /// The resolution asked for.
        dpi: u16,
        /// The resolutions both the scanner and the app offer.
        supported: Vec<u16>,
    },
    /// The scan settings ask for a colour mode that the scanner does not report.
    #[error("{}", color_mode_message(*color, supported))]
    UnsupportedColorMode {
        /// The colour mode asked for.
        color: ColorMode,
        /// The colour modes the scanner reports.
        supported: Vec<ColorMode>,
    },
    /// The document is not well-formed XML.
    #[error("the printer sent malformed XML: {0}")]
    Xml(String),
    /// The document's root element is not the one asked for.
    #[error("the printer answered with {found} where {expected} was expected")]
    UnexpectedRoot {
        /// The root element expected.
        expected: &'static str,
        /// The root element found.
        found: String,
    },
    /// A required element is absent; the value is its path by local names from the root.
    #[error("the printer's answer has no {0} element")]
    Missing(&'static str),
    /// An element that holds a number holds something else.
    #[error("the printer's answer has {value:?} in {path} where a number belongs")]
    BadNumber {
        /// The element's path by local names from the root.
        path: &'static str,
        /// Its text.
        value: String,
    },
    /// A `Location` that is not an eSCL scan job.
    #[error("the printer gave the scan job an unusable location {0:?}")]
    BadJobLocation(String),
}

fn resolution_message(dpi: u16, supported: &[u16]) -> String {
    if supported.is_empty() {
        format!(
            "the scanner offers none of this app's resolutions ({} dpi)",
            join(&RESOLUTIONS)
        )
    } else {
        format!(
            "the scanner cannot scan at {dpi} dpi; choose one of {} dpi",
            join(supported)
        )
    }
}

fn color_mode_message(color: ColorMode, supported: &[ColorMode]) -> String {
    match supported.first() {
        Some(other) => format!("the scanner cannot scan in {color}; choose {other}"),
        None => "the scanner offers neither grey nor colour scanning".to_owned(),
    }
}

fn join(values: &[u16]) -> String {
    values
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The `ScanSettings` document for one A4 flatbed page, as the M130a accepted it.
///
/// The region is A4 at offset 0,0, clamped to the scanner's maximum.
///
/// # Errors
///
/// [`EsclError::UnsupportedResolution`] or [`EsclError::UnsupportedColorMode`] when `settings`
/// ask for something `caps` or [`RESOLUTIONS`] do not include.
pub fn scan_settings_xml(
    settings: &ScanSettings,
    caps: &Capabilities,
) -> Result<String, EsclError> {
    if !RESOLUTIONS.contains(&settings.dpi) || !caps.resolutions.contains(&settings.dpi) {
        return Err(EsclError::UnsupportedResolution {
            dpi: settings.dpi,
            supported: RESOLUTIONS
                .into_iter()
                .filter(|dpi| caps.resolutions.contains(dpi))
                .collect(),
        });
    }
    if !caps.color_modes.contains(&settings.color) {
        return Err(EsclError::UnsupportedColorMode {
            color: settings.color,
            supported: caps.color_modes.clone(),
        });
    }
    let width = A4_WIDTH.min(caps.max_width);
    let height = A4_HEIGHT.min(caps.max_height);
    let mode = settings.color.escl_name();
    let dpi = settings.dpi;
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
{SCAN_SETTINGS_START}
  <pwg:Version>2.0</pwg:Version>
  <scan:Intent>Document</scan:Intent>
  <pwg:ScanRegions>
    <pwg:ScanRegion>
      <pwg:Height>{height}</pwg:Height>
      <pwg:ContentRegionUnits>escl:ThreeHundredthsOfInches</pwg:ContentRegionUnits>
      <pwg:Width>{width}</pwg:Width>
      <pwg:XOffset>0</pwg:XOffset>
      <pwg:YOffset>0</pwg:YOffset>
    </pwg:ScanRegion>
  </pwg:ScanRegions>
  <pwg:InputSource>Platen</pwg:InputSource>
  <scan:ColorMode>{mode}</scan:ColorMode>
  <scan:XResolution>{dpi}</scan:XResolution>
  <scan:YResolution>{dpi}</scan:YResolution>
  <pwg:DocumentFormat>image/jpeg</pwg:DocumentFormat>
</scan:ScanSettings>
"#
    ))
}

/// Reads `GET /eSCL/ScannerCapabilities`, keeping what the flatbed offers.
///
/// # Errors
///
/// [`EsclError`] when the XML is malformed, its root is not `ScannerCapabilities`, or the model,
/// the platen's limits or a resolution are missing or not numbers.
pub fn parse_capabilities(xml: &[u8]) -> Result<Capabilities, EsclError> {
    let root = parse_document(xml, "ScannerCapabilities")?;
    let make_and_model = root.at("ScannerCapabilities/MakeAndModel")?.text();
    let platen = root.at("ScannerCapabilities/Platen/PlatenInputCaps")?;
    let profiles: Vec<&Element> = platen
        .children("SettingProfiles")
        .flat_map(|profiles| profiles.children("SettingProfile"))
        .collect();
    Ok(Capabilities {
        make_and_model,
        max_width: platen.number_at("ScannerCapabilities/Platen/PlatenInputCaps/MaxWidth")?,
        max_height: platen.number_at("ScannerCapabilities/Platen/PlatenInputCaps/MaxHeight")?,
        resolutions: resolutions(&profiles)?,
        color_modes: color_modes(&profiles),
    })
}

/// The path of an element inside a `DiscreteResolution`.
macro_rules! discrete_resolution {
    ($leaf:literal) => {
        concat!(
            "ScannerCapabilities/Platen/PlatenInputCaps/SettingProfiles/SettingProfile/",
            "SupportedResolutions/DiscreteResolutions/DiscreteResolution/",
            $leaf
        )
    };
}

fn resolutions(profiles: &[&Element]) -> Result<Vec<u16>, EsclError> {
    let mut found = Vec::new();
    for resolution in profiles
        .iter()
        .flat_map(|profile| profile.children("SupportedResolutions"))
        .flat_map(|supported| supported.children("DiscreteResolutions"))
        .flat_map(|discrete| discrete.children("DiscreteResolution"))
    {
        let x: u16 = resolution.number_at(discrete_resolution!("XResolution"))?;
        let y: u16 = resolution.number_at(discrete_resolution!("YResolution"))?;
        if x == y {
            found.push(x);
        }
    }
    found.sort_unstable();
    found.dedup();
    Ok(found)
}

fn color_modes(profiles: &[&Element]) -> Vec<ColorMode> {
    let mut found = Vec::new();
    for mode in profiles
        .iter()
        .flat_map(|profile| profile.children("ColorModes"))
        .flat_map(|modes| modes.children("ColorMode"))
        .filter_map(|mode| ColorMode::from_escl_name(&mode.text()))
    {
        if !found.contains(&mode) {
            found.push(mode);
        }
    }
    found
}

/// Reads `GET /eSCL/ScannerStatus`.
///
/// # Errors
///
/// [`EsclError`] when the XML is malformed, its root is not `ScannerStatus`, the state is
/// missing, or a job lacks its URI, state or image count.
pub fn parse_status(xml: &[u8]) -> Result<ScannerStatus, EsclError> {
    let root = parse_document(xml, "ScannerStatus")?;
    let state = ScannerState::from_pwg(root.at("ScannerStatus/State")?.text());
    let jobs = root
        .children("Jobs")
        .flat_map(|jobs| jobs.children("JobInfo"))
        .map(job_info)
        .collect::<Result<_, _>>()?;
    Ok(ScannerStatus { state, jobs })
}

fn job_info(job: &Element) -> Result<JobInfo, EsclError> {
    Ok(JobInfo {
        uri: job.text_at("ScannerStatus/Jobs/JobInfo/JobUri")?,
        state: job.text_at("ScannerStatus/Jobs/JobInfo/JobState")?,
        images_completed: job.number_at("ScannerStatus/Jobs/JobInfo/ImagesCompleted")?,
    })
}

/// The path of the job a `ScanJobs` POST created, from its `Location` header.
///
/// A relative location is kept; an absolute `http://host[:port]/…` one gives its path, since
/// over USB the host and port mean nothing. A trailing `/` is dropped so that the caller can
/// append `/NextDocument`.
///
/// # Errors
///
/// [`EsclError::BadJobLocation`] unless the path is `/eSCL/ScanJobs/` followed by one segment
/// of URI-safe characters (which also keeps CR, LF and spaces out of later request lines).
pub fn job_path(location: &str) -> Result<String, EsclError> {
    const PREFIX: &str = "/eSCL/ScanJobs/";
    let bad = || EsclError::BadJobLocation(location.to_owned());
    let path = location_path(location.trim()).ok_or_else(bad)?;
    let path = path.strip_suffix('/').unwrap_or(path);
    let id = path.strip_prefix(PREFIX).ok_or_else(bad)?;
    let safe = |c: char| c.is_ascii_alphanumeric() || "-._~:".contains(c);
    if id.is_empty() || !id.chars().all(safe) {
        return Err(bad());
    }
    Ok(path.to_owned())
}

/// The path of a location: all of a relative one, or what follows the authority of an
/// `http`/`https` one (scheme in any case); `None` for an absolute location without a path.
fn location_path(location: &str) -> Option<&str> {
    let after_scheme = ["http://", "https://"].iter().find_map(|scheme| {
        let (head, rest) = location.split_at_checked(scheme.len())?;
        head.eq_ignore_ascii_case(scheme).then_some(rest)
    });
    match after_scheme {
        Some(rest) => rest.find('/').map(|slash| &rest[slash..]),
        None => Some(location),
    }
}

/// An XML element reduced to what eSCL documents use: local name, text and children.
#[derive(Debug, Default)]
struct Element {
    name: String,
    text: String,
    children: Vec<Element>,
}

impl Element {
    fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |child| child.name == name)
    }

    fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.name == name)
    }

    fn text(&self) -> String {
        self.text.trim().to_owned()
    }

    /// The descendant at `path`, whose first segment names this (root) element.
    fn at(&self, path: &'static str) -> Result<&Element, EsclError> {
        path.split('/').skip(1).try_fold(self, |element, name| {
            element.child(name).ok_or(EsclError::Missing(path))
        })
    }

    /// The text of the child named by the last segment of `path`, a full path that ends at
    /// one of this element's children.
    fn text_at(&self, path: &'static str) -> Result<String, EsclError> {
        let name = path.rsplit('/').next().unwrap_or(path);
        self.child(name)
            .map(Element::text)
            .ok_or(EsclError::Missing(path))
    }

    /// Like [`Element::text_at`], parsed as a number.
    fn number_at<N: FromStr>(&self, path: &'static str) -> Result<N, EsclError> {
        let value = self.text_at(path)?;
        value
            .parse()
            .map_err(|_| EsclError::BadNumber { path, value })
    }
}

fn parse_document(xml: &[u8], expected: &'static str) -> Result<Element, EsclError> {
    let root = parse_tree(xml)?;
    if root.name == expected {
        Ok(root)
    } else {
        Err(EsclError::UnexpectedRoot {
            expected,
            found: root.name,
        })
    }
}

/// eSCL documents are a few KiB and nest about ten deep; these bounds keep a broken or hostile
/// answer from exhausting memory, or the stack when the tree is dropped.
const MAX_DOCUMENT: usize = 1 << 20;
const MAX_DEPTH: usize = 32;

fn parse_tree(xml: &[u8]) -> Result<Element, EsclError> {
    if xml.len() > MAX_DOCUMENT {
        return Err(EsclError::Xml(
            "the document is larger than 1 MiB".to_owned(),
        ));
    }
    let mut reader = Reader::from_reader(xml);
    let mut tree = TreeBuilder::default();
    loop {
        let event = reader.read_event().map_err(|error| {
            EsclError::Xml(format!("{error} at byte {}", reader.error_position()))
        })?;
        if let Event::Eof = event {
            return tree.finish();
        }
        tree.add(event)?;
    }
}

/// Builds the [`Element`] tree from reader events.
#[derive(Debug, Default)]
struct TreeBuilder {
    open: Vec<Element>,
    root: Option<Element>,
}

impl TreeBuilder {
    fn add(&mut self, event: Event<'_>) -> Result<(), EsclError> {
        match event {
            Event::Start(start) => self.open(start.local_name().as_ref()),
            Event::Empty(start) => {
                self.open(start.local_name().as_ref())?;
                self.close()
            }
            Event::End(_) => self.close(),
            other => self.text(&text_of(other)?),
        }
    }

    fn open(&mut self, name: &str) -> Result<(), EsclError> {
        if self.root.is_some() {
            return Err(EsclError::Xml(format!(
                "element {name} after the root element"
            )));
        }
        if self.open.len() >= MAX_DEPTH {
            return Err(EsclError::Xml(format!(
                "elements nested deeper than {MAX_DEPTH}"
            )));
        }
        self.open.push(Element {
            name: name.to_owned(),
            ..Element::default()
        });
        Ok(())
    }

    fn close(&mut self) -> Result<(), EsclError> {
        let element = self
            .open
            .pop()
            .ok_or_else(|| EsclError::Xml("end tag without a start tag".to_owned()))?;
        match self.open.last_mut() {
            Some(parent) => parent.children.push(element),
            None => self.root = Some(element),
        }
        Ok(())
    }

    fn text(&mut self, text: &str) -> Result<(), EsclError> {
        match self.open.last_mut() {
            Some(element) => element.text.push_str(text),
            None if text.trim().is_empty() => {}
            None => {
                return Err(EsclError::Xml(format!(
                    "text {text:?} outside the root element"
                )));
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<Element, EsclError> {
        if let Some(element) = self.open.last() {
            return Err(EsclError::Xml(format!(
                "the document ends inside element {}",
                element.name
            )));
        }
        self.root
            .ok_or_else(|| EsclError::Xml("the document has no root element".to_owned()))
    }
}

/// The character data an event carries; none for comments, declarations and the like.
fn text_of(event: Event<'_>) -> Result<Cow<'_, str>, EsclError> {
    Ok(match event {
        Event::Text(text) => text.xml10_content(),
        Event::CData(data) => data.xml10_content(),
        Event::GeneralRef(reference) => Cow::Owned(resolve(&reference)?),
        Event::Start(_)
        | Event::Empty(_)
        | Event::End(_)
        | Event::Comment(_)
        | Event::Decl(_)
        | Event::PI(_)
        | Event::DocType(_)
        | Event::Eof => Cow::Borrowed(""),
    })
}

fn resolve(reference: &BytesRef<'_>) -> Result<String, EsclError> {
    let unknown = || EsclError::Xml(format!("unknown entity &{};", &**reference));
    match reference.resolve_char_ref() {
        Ok(Some(c)) => Ok(c.to_string()),
        Ok(None) => resolve_predefined_entity(reference)
            .map(str::to_owned)
            .ok_or_else(unknown),
        Err(error) => Err(EsclError::Xml(error.to_string())),
    }
}

#[cfg(test)]
mod tests;
