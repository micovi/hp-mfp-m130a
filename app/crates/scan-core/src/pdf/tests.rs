use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId};

use super::*;
use crate::jpeg::tests::{adobe, jpeg, sof0};
use crate::testing::fixture;

fn page(jpeg: Vec<u8>, dpi: u16) -> Page {
    Page::new(jpeg, dpi).unwrap()
}

fn grey_75() -> Page {
    page(fixture("synthetic-grey-75dpi.jpg").unwrap(), 75)
}

fn colour_150() -> Page {
    page(fixture("synthetic-colour-150dpi.jpg").unwrap(), 150)
}

fn read_back(pages: &[Page]) -> Document {
    let pdf = write_pdf(pages).unwrap();
    assert!(pdf.starts_with(b"%PDF-1."));
    assert!(pdf.trim_ascii_end().ends_with(b"%%EOF"));
    let document = Document::load_mem(&pdf).unwrap();
    let tree = document
        .catalog()
        .unwrap()
        .get_deref(b"Pages", &document)
        .unwrap()
        .as_dict()
        .unwrap();
    let count = tree.get(b"Count").unwrap().as_i64().unwrap();
    assert_eq!(usize::try_from(count).unwrap(), pages.len());
    document
}

fn page_ids(document: &Document) -> Vec<ObjectId> {
    document.get_pages().into_values().collect()
}

fn numbers(object: &Object) -> Vec<f32> {
    object
        .as_array()
        .unwrap()
        .iter()
        .map(|number| number.as_float().unwrap())
        .collect()
}

fn assert_close(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len(), "{actual:?} vs {expected:?}");
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 0.001, "{actual:?} vs {expected:?}");
    }
}

fn media_box(document: &Document, page: ObjectId) -> Vec<f32> {
    numbers(
        document
            .get_dictionary(page)
            .unwrap()
            .get(b"MediaBox")
            .unwrap(),
    )
}

/// The one image `XObject` a page uses, with its resource name.
fn image(document: &Document, page: ObjectId) -> (Vec<u8>, &lopdf::Stream) {
    let resources = document
        .get_dictionary(page)
        .unwrap()
        .get_deref(b"Resources", document)
        .unwrap()
        .as_dict()
        .unwrap();
    let x_objects = resources
        .get_deref(b"XObject", document)
        .unwrap()
        .as_dict()
        .unwrap();
    assert_eq!(x_objects.len(), 1);
    let (name, reference) = x_objects.iter().next().unwrap();
    let stream = document
        .get_object(reference.as_reference().unwrap())
        .unwrap()
        .as_stream()
        .unwrap();
    (name.clone(), stream)
}

fn name<'a>(dict: &'a Dictionary, key: &[u8]) -> &'a [u8] {
    dict.get(key).unwrap().as_name().unwrap()
}

fn assert_image(stream: &lopdf::Stream, jpeg: &[u8], size: (i64, i64), color_space: &[u8]) {
    let dict = &stream.dict;
    assert_eq!(name(dict, b"Type"), b"XObject");
    assert_eq!(name(dict, b"Subtype"), b"Image");
    assert_eq!(name(dict, b"Filter"), b"DCTDecode");
    assert_eq!(name(dict, b"ColorSpace"), color_space);
    assert_eq!(dict.get(b"BitsPerComponent").unwrap().as_i64().unwrap(), 8);
    assert_eq!(dict.get(b"Width").unwrap().as_i64().unwrap(), size.0);
    assert_eq!(dict.get(b"Height").unwrap().as_i64().unwrap(), size.1);
    assert_eq!(stream.content, jpeg);
}

/// The page's content: draw the named image scaled over the whole page.
fn assert_draws_full_page(document: &Document, page: ObjectId, image_name: &[u8], size: [f32; 2]) {
    let content = Content::decode(&document.get_page_content(page)).unwrap();
    let operators: Vec<&str> = content
        .operations
        .iter()
        .map(|operation| operation.operator.as_str())
        .collect();
    assert_eq!(operators, ["q", "cm", "Do", "Q"]);
    let matrix: Vec<f32> = content.operations[1]
        .operands
        .iter()
        .map(|number| number.as_float().unwrap())
        .collect();
    assert_close(&matrix, &[size[0], 0.0, 0.0, size[1], 0.0, 0.0]);
    assert_eq!(
        content.operations[2].operands,
        [Object::Name(image_name.to_vec())]
    );
}

#[test]
fn writes_a_75_dpi_grey_page_at_its_physical_size() {
    let grey = grey_75();
    let document = read_back(std::slice::from_ref(&grey));
    let pages = page_ids(&document);
    assert_eq!(pages.len(), 1);
    assert_close(&media_box(&document, pages[0]), &[0.0, 0.0, 595.2, 841.92]);
    let (image_name, stream) = image(&document, pages[0]);
    assert_image(stream, grey.jpeg(), (620, 877), b"DeviceGray");
    assert!(stream.dict.get(b"Decode").is_err());
    assert_draws_full_page(&document, pages[0], &image_name, [595.2, 841.92]);
}

#[test]
fn writes_pages_in_order_with_their_own_size_and_colour_space() {
    let colour = colour_150();
    let grey = page(fixture("synthetic-grey-75dpi.jpg").unwrap(), 300);
    let document = read_back(&[colour.clone(), grey.clone(), colour.clone()]);
    let pages = page_ids(&document);
    assert_eq!(pages.len(), 3);

    let (colour_size, grey_size) = ([595.2, 841.92], [148.8, 210.48]);
    for (index, (page, size)) in [
        (&colour, colour_size),
        (&grey, grey_size),
        (&colour, colour_size),
    ]
    .into_iter()
    .enumerate()
    {
        let id = pages[index];
        assert_close(&media_box(&document, id), &[0.0, 0.0, size[0], size[1]]);
        let (image_name, stream) = image(&document, id);
        let (space, pixels): (&[u8], _) = if page.info().components == 3 {
            (b"DeviceRGB", (1240, 1754))
        } else {
            (b"DeviceGray", (620, 877))
        };
        assert_image(stream, page.jpeg(), pixels, space);
        assert_draws_full_page(&document, id, &image_name, size);
    }
}

fn image_dict(segments: &[Vec<u8>]) -> Dictionary {
    let document = read_back(&[page(jpeg(segments), 72)]);
    let page_id = page_ids(&document)[0];
    image(&document, page_id).1.dict.clone()
}

#[test]
fn inverts_cmyk_only_when_adobe_says_untransformed() {
    let dict = image_dict(&[adobe(0), sof0(30, 20, 4)]);
    assert_eq!(name(&dict, b"ColorSpace"), b"DeviceCMYK");
    assert_close(
        &numbers(dict.get(b"Decode").unwrap()),
        &[1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0],
    );
    assert_eq!(dict.get(b"Width").unwrap().as_i64().unwrap(), 20);

    for segments in [
        vec![sof0(30, 20, 4)],
        vec![adobe(2), sof0(30, 20, 4)],
        vec![adobe(1), sof0(30, 20, 4)],
    ] {
        let dict = image_dict(&segments);
        assert_eq!(name(&dict, b"ColorSpace"), b"DeviceCMYK");
        assert!(dict.get(b"Decode").is_err());
    }
}

#[test]
fn an_adobe_marker_does_not_invert_grey_or_rgb() {
    for (components, space) in [(1, &b"DeviceGray"[..]), (3, b"DeviceRGB")] {
        let dict = image_dict(&[adobe(0), sof0(30, 20, components)]);
        assert_eq!(name(&dict, b"ColorSpace"), space);
        assert!(dict.get(b"Decode").is_err());
    }
}

#[test]
fn a_page_keeps_its_jpeg_and_resolution_and_reads_the_header() {
    let bytes = fixture("synthetic-grey-75dpi.jpg").unwrap();
    let grey = Page::new(bytes.clone(), 75).unwrap();
    assert_eq!(grey.jpeg(), bytes);
    assert_eq!(grey.dpi(), 75);
    assert_eq!(
        grey.info(),
        &JpegInfo {
            width: 620,
            height: 877,
            components: 1,
            progressive: false,
        }
    );
}

#[test]
fn a_page_needs_a_usable_jpeg() {
    let error = Page::new(b"not a jpeg".to_vec(), 75).unwrap_err();
    assert_eq!(error, PageError::Jpeg(JpegError::NotJpeg));
    assert_eq!(error.to_string(), "the page is not a JPEG image");
    let arithmetic = jpeg(&[crate::jpeg::tests::frame(0xC9, 8, 1, 1, 1)]);
    assert_eq!(
        Page::new(arithmetic, 75),
        Err(PageError::Jpeg(JpegError::Unsupported(
            "arithmetic coding".to_owned()
        )))
    );
}

#[test]
fn a_page_needs_a_resolution() {
    let bytes = fixture("synthetic-grey-75dpi.jpg").unwrap();
    let error = Page::new(bytes, 0).unwrap_err();
    assert_eq!(error, PageError::NoResolution);
    assert_eq!(
        error.to_string(),
        "the page has no resolution (0 dpi), so its size is unknown"
    );
    assert_eq!(page(jpeg(&[sof0(1, 1, 1)]), 1).dpi(), 1);
}

#[test]
fn writes_borrowed_pages_like_owned_ones() {
    let (grey, colour) = (grey_75(), colour_150());
    let owned = write_pdf(&[grey.clone(), colour.clone()]).unwrap();
    assert_eq!(write_pdf(&[&grey, &colour]).unwrap(), owned);
    assert_eq!(read_back(&[grey, colour]).get_pages().len(), 2);
}

#[test]
fn no_pages_is_an_error() {
    assert_eq!(write_pdf::<Page>(&[]), Err(PdfError::NoPages));
    assert_eq!(PdfError::NoPages.to_string(), "there are no pages to save");
}

#[test]
fn refuses_more_pages_than_one_pdf_holds() {
    let tiny = page(jpeg(&[sof0(1, 1, 1)]), 72);
    let at_limit = vec![&tiny; 10_000];
    let pdf = write_pdf(&at_limit).unwrap();
    assert_eq!(Document::load_mem(&pdf).unwrap().get_pages().len(), 10_000);
    let mut over = at_limit;
    over.push(&tiny);
    let error = write_pdf(&over).unwrap_err();
    assert_eq!(error, PdfError::TooManyPages(10_001));
    assert_eq!(
        error.to_string(),
        "10001 pages are more than the 10000 one PDF can hold; save fewer pages"
    );
}
