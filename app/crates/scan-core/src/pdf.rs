//! Scanned pages written into one PDF, each JPEG embedded as is.

use std::borrow::Borrow;

use pdf_writer::{Content, Filter, Name, Pdf, Rect, Ref};

use crate::jpeg::{self, Components, Header, JpegError, JpegInfo};

/// One scanned page: a JPEG a PDF can embed, and the resolution it was scanned at.
///
/// [`Page::new`] checks both, so every `Page` can be written by [`write_pdf`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    jpeg: Vec<u8>,
    dpi: u16,
    info: JpegInfo,
    header: Header,
}

/// Why scanned bytes cannot be a page.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PageError {
    /// The JPEG is unusable.
    #[error(transparent)]
    Jpeg(#[from] JpegError),
    /// A resolution of 0 dpi.
    #[error("the page has no resolution (0 dpi), so its size is unknown")]
    NoResolution,
}

impl Page {
    /// A page from the JPEG the scanner sent, scanned at `dpi`.
    ///
    /// # Errors
    ///
    /// [`PageError::NoResolution`] for 0 dpi, [`PageError::Jpeg`] when the JPEG's header is
    /// unusable (see [`jpeg_info`](crate::jpeg::jpeg_info)).
    pub fn new(jpeg: Vec<u8>, dpi: u16) -> Result<Self, PageError> {
        if dpi == 0 {
            return Err(PageError::NoResolution);
        }
        let header = jpeg::header(&jpeg)?;
        Ok(Self {
            jpeg,
            dpi,
            info: header.info(),
            header,
        })
    }

    /// The JPEG exactly as received.
    #[must_use]
    pub fn jpeg(&self) -> &[u8] {
        &self.jpeg
    }

    /// Dots per inch, which turns pixels into a physical page size.
    #[must_use]
    pub fn dpi(&self) -> u16 {
        self.dpi
    }

    /// What the JPEG's frame header says.
    #[must_use]
    pub fn info(&self) -> &JpegInfo {
        &self.info
    }
}

/// Why the pages could not be written as a PDF.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PdfError {
    /// There is nothing to write.
    #[error("there are no pages to save")]
    NoPages,
    /// More pages than one PDF from this app holds.
    #[error("{0} pages are more than the {MAX_PAGES} one PDF can hold; save fewer pages")]
    TooManyPages(usize),
}

/// Far more than one sitting at a flatbed produces, and it keeps every PDF object number (three
/// per page) well inside the `i32` that PDF object numbers are written as.
const MAX_PAGES: usize = 10_000;

/// Points per inch.
const POINTS: f32 = 72.0;

/// The image's name in each page's resources.
const IMAGE: Name<'static> = Name(b"Im1");

/// Writes the pages, in order, as one PDF.
///
/// Each page is the physical size of its scan (pixels ÷ dpi × 72 pt) and shows its JPEG
/// unchanged through `DCTDecode`, in `DeviceGray`, `DeviceRGB` or `DeviceCMYK` by the JPEG's
/// component count. A CMYK JPEG with an Adobe APP14 marker saying "no transform" holds inverted
/// samples, as Adobe applications write them, so its image gets `/Decode [1 0 1 0 1 0 1 0]`.
///
/// Takes owned pages or references (`&[Page]` or `&[&Page]`), so callers need not copy JPEGs.
///
/// # Errors
///
/// [`PdfError::NoPages`] for no pages and [`PdfError::TooManyPages`] over 10 000 pages.
pub fn write_pdf<P: Borrow<Page>>(pages: &[P]) -> Result<Vec<u8>, PdfError> {
    if pages.is_empty() {
        return Err(PdfError::NoPages);
    }
    if pages.len() > MAX_PAGES {
        return Err(PdfError::TooManyPages(pages.len()));
    }
    let mut pdf = Pdf::new();
    let mut ids = Ref::new(1);
    let catalog = ids.bump();
    let tree = ids.bump();
    let mut kids = Vec::with_capacity(pages.len());
    let mut count = 0;
    for page in pages {
        let refs = PageRefs {
            tree,
            page: ids.bump(),
            image: ids.bump(),
            content: ids.bump(),
        };
        write_page(&mut pdf, &refs, page.borrow());
        kids.push(refs.page);
        count += 1;
    }
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree).kids(kids).count(count);
    Ok(pdf.finish())
}

struct PageRefs {
    tree: Ref,
    page: Ref,
    image: Ref,
    content: Ref,
}

fn write_page(pdf: &mut Pdf, refs: &PageRefs, page: &Page) {
    let header = page.header;
    let dpi = f32::from(page.dpi);
    let width = f32::from(header.width) * POINTS / dpi;
    let height = f32::from(header.height) * POINTS / dpi;

    let mut image = pdf.image_xobject(refs.image, &page.jpeg);
    image.filter(Filter::DctDecode);
    image
        .width(i32::from(header.width))
        .height(i32::from(header.height))
        .bits_per_component(8);
    match header.components {
        Components::Grey => image.color_space().device_gray(),
        Components::Rgb => image.color_space().device_rgb(),
        Components::Cmyk => {
            image.color_space().device_cmyk();
            if header.adobe_transform == Some(0) {
                image.decode([1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0]);
            }
        }
    }
    drop(image);

    let mut content = Content::new();
    content
        .save_state()
        .transform([width, 0.0, 0.0, height, 0.0, 0.0])
        .x_object(IMAGE)
        .restore_state();
    pdf.stream(refs.content, &content.finish());

    let mut pdf_page = pdf.page(refs.page);
    pdf_page
        .parent(refs.tree)
        .media_box(Rect::new(0.0, 0.0, width, height))
        .contents(refs.content);
    pdf_page.resources().x_objects().pair(IMAGE, refs.image);
}

#[cfg(test)]
mod tests;
