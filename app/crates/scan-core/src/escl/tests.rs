use std::fmt::Write as _;

use super::*;
use crate::testing::fixture;

fn m130a_capabilities() -> Capabilities {
    parse_capabilities(&fixture("m130a-scanner-capabilities.xml").unwrap()).unwrap()
}

fn status_fixture(name: &str) -> ScannerStatus {
    parse_status(&fixture(&format!("m130a-scanner-status-{name}.xml")).unwrap()).unwrap()
}

/// The body of the `ScanJobs` POST the printer accepted.
fn golden_scan_settings() -> String {
    let raw = fixture("m130a-scanjobs-request.http").unwrap();
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    String::from_utf8(raw[end + 4..].to_vec()).unwrap()
}

fn caps_document(platen_caps: &str) -> String {
    format!(
        r#"<scan:ScannerCapabilities xmlns:scan="s" xmlns:pwg="p">
             <pwg:MakeAndModel>Test</pwg:MakeAndModel>
             <scan:Platen><scan:PlatenInputCaps>{platen_caps}</scan:PlatenInputCaps></scan:Platen>
           </scan:ScannerCapabilities>"#
    )
}

const LIMITS: &str = "<scan:MaxWidth>2550</scan:MaxWidth><scan:MaxHeight>3508</scan:MaxHeight>";

fn caps_with(limits: &str, profile: &str) -> Result<Capabilities, EsclError> {
    let platen = format!(
        "{limits}<scan:SettingProfiles><scan:SettingProfile>{profile}\
         </scan:SettingProfile></scan:SettingProfiles>"
    );
    parse_capabilities(caps_document(&platen).as_bytes())
}

const GREY_AND_COLOUR: &str = "<scan:ColorModes><scan:ColorMode>Grayscale8</scan:ColorMode>\
     <scan:ColorMode>RGB24</scan:ColorMode></scan:ColorModes>";

fn resolutions(pairs: &[(u32, u32)]) -> String {
    let mut xml = String::from("<scan:SupportedResolutions><scan:DiscreteResolutions>");
    for (x, y) in pairs {
        write!(
            xml,
            "<scan:DiscreteResolution><scan:XResolution>{x}</scan:XResolution>\
             <scan:YResolution>{y}</scan:YResolution></scan:DiscreteResolution>"
        )
        .unwrap();
    }
    xml + "</scan:DiscreteResolutions></scan:SupportedResolutions>"
}

fn caps(resolutions: Vec<u16>, color_modes: Vec<ColorMode>) -> Capabilities {
    Capabilities {
        make_and_model: "Test".to_owned(),
        max_width: 2550,
        max_height: 3508,
        resolutions,
        color_modes,
    }
}

const GREY_75: ScanSettings = ScanSettings {
    color: ColorMode::Grey,
    dpi: 75,
};

#[test]
fn builds_the_scan_settings_the_printer_accepted_byte_for_byte() {
    assert_eq!(
        scan_settings_xml(&GREY_75, &m130a_capabilities()).unwrap(),
        golden_scan_settings()
    );
}

#[test]
fn colour_and_resolution_change_only_their_elements() {
    let settings = ScanSettings {
        color: ColorMode::Colour,
        dpi: 300,
    };
    let expected = golden_scan_settings()
        .replace("Grayscale8", "RGB24")
        .replace(">75<", ">300<");
    assert_eq!(
        scan_settings_xml(&settings, &m130a_capabilities()).unwrap(),
        expected
    );
}

#[test]
fn every_app_resolution_the_m130a_reports_is_accepted() {
    for dpi in [75, 150, 300, 600] {
        for color in [ColorMode::Grey, ColorMode::Colour] {
            let xml = scan_settings_xml(&ScanSettings { color, dpi }, &m130a_capabilities());
            assert!(xml.unwrap().contains(&format!("<scan:XResolution>{dpi}<")));
        }
    }
}

#[test]
fn clamps_the_a4_region_to_the_reported_maximum() {
    let small = Capabilities {
        max_width: 2479,
        max_height: 3507,
        ..caps(vec![75], vec![ColorMode::Grey])
    };
    let xml = scan_settings_xml(&GREY_75, &small).unwrap();
    assert!(xml.contains("<pwg:Width>2479</pwg:Width>"));
    assert!(xml.contains("<pwg:Height>3507</pwg:Height>"));

    let exact = Capabilities {
        max_width: 2480,
        max_height: 3508,
        ..small
    };
    assert_eq!(
        scan_settings_xml(&GREY_75, &exact).unwrap(),
        golden_scan_settings()
    );
}

#[test]
fn rejects_a_resolution_the_scanner_does_not_report() {
    let error = scan_settings_xml(&GREY_75, &caps(vec![150, 300], vec![ColorMode::Grey]));
    assert_eq!(
        error,
        Err(EsclError::UnsupportedResolution {
            dpi: 75,
            supported: vec![150, 300],
        })
    );
    assert_eq!(
        error.unwrap_err().to_string(),
        "the scanner cannot scan at 75 dpi; choose one of 150, 300 dpi"
    );
}

#[test]
fn rejects_resolutions_the_app_does_not_offer_even_when_reported() {
    for dpi in [0, 100, 200, 1200] {
        let settings = ScanSettings {
            color: ColorMode::Grey,
            dpi,
        };
        assert_eq!(
            scan_settings_xml(&settings, &m130a_capabilities()),
            Err(EsclError::UnsupportedResolution {
                dpi,
                supported: vec![75, 150, 300, 600],
            })
        );
    }
}

#[test]
fn rejects_a_colour_mode_the_scanner_does_not_report() {
    let grey_only = caps(vec![75], vec![ColorMode::Grey]);
    let colour = ScanSettings {
        color: ColorMode::Colour,
        dpi: 75,
    };
    let error = scan_settings_xml(&colour, &grey_only).unwrap_err();
    assert_eq!(
        error,
        EsclError::UnsupportedColorMode {
            color: ColorMode::Colour,
            supported: vec![ColorMode::Grey],
        }
    );
    assert_eq!(
        error.to_string(),
        "the scanner cannot scan in colour; choose grey"
    );
    let colour_only = caps(vec![75], vec![ColorMode::Colour]);
    assert_eq!(
        scan_settings_xml(&GREY_75, &colour_only)
            .unwrap_err()
            .to_string(),
        "the scanner cannot scan in grey; choose colour"
    );
}

#[test]
fn says_so_when_the_scanner_offers_no_colour_mode_the_app_knows() {
    let error = scan_settings_xml(&GREY_75, &caps(vec![75], Vec::new())).unwrap_err();
    assert_eq!(
        error,
        EsclError::UnsupportedColorMode {
            color: ColorMode::Grey,
            supported: Vec::new(),
        }
    );
    assert_eq!(
        error.to_string(),
        "the scanner offers neither grey nor colour scanning"
    );
}

#[test]
fn says_so_when_the_scanner_offers_none_of_the_app_resolutions() {
    let error = scan_settings_xml(&GREY_75, &caps(vec![200, 1200], vec![ColorMode::Grey]));
    let error = error.unwrap_err();
    assert_eq!(
        error,
        EsclError::UnsupportedResolution {
            dpi: 75,
            supported: Vec::new(),
        }
    );
    assert_eq!(
        error.to_string(),
        "the scanner offers none of this app's resolutions (75, 150, 300, 600 dpi)"
    );
}

#[test]
fn parses_the_m130a_capabilities() {
    assert_eq!(
        m130a_capabilities(),
        Capabilities {
            make_and_model: "HP LaserJet MFP M130a".to_owned(),
            max_width: 2550,
            max_height: 3508,
            resolutions: vec![75, 150, 200, 300, 600, 1200],
            color_modes: vec![ColorMode::Colour, ColorMode::Grey],
        }
    );
}

#[test]
fn parses_capabilities_whatever_the_prefixes_whitespace_and_comments() {
    let xml = "<?xml version='1.0'?>
        <!-- a comment before the root -->
        <ScannerCapabilities xmlns='http://schemas.hp.com/imaging/escl/2011/05/03'
                             xmlns:p='http://www.pwg.org/schemas/2010/12/sm'>
          <p:MakeAndModel>
            HP  LaserJet
          </p:MakeAndModel>
          <Platen>
            <PlatenInputCaps>
              <MaxWidth> 2550 </MaxWidth> <!-- inside -->
              <MaxHeight>3508</MaxHeight>
              <SettingProfiles><SettingProfile>
                <ColorModes><ColorMode>Grayscale8</ColorMode></ColorModes>
                <SupportedResolutions><DiscreteResolutions><DiscreteResolution>
                  <XResolution>300</XResolution><YResolution>300</YResolution>
                </DiscreteResolution></DiscreteResolutions></SupportedResolutions>
              </SettingProfile></SettingProfiles>
            </PlatenInputCaps>
          </Platen>
        </ScannerCapabilities>
        <!-- and after -->
    ";
    assert_eq!(
        parse_capabilities(xml.as_bytes()).unwrap(),
        Capabilities {
            make_and_model: "HP  LaserJet".to_owned(),
            max_width: 2550,
            max_height: 3508,
            resolutions: vec![300],
            color_modes: vec![ColorMode::Grey],
        }
    );
}

#[test]
fn merges_profiles_skips_unknown_modes_and_unequal_resolutions() {
    let first = format!(
        "<scan:ColorModes><scan:ColorMode>BlackAndWhite1</scan:ColorMode>\
         <scan:ColorMode>Grayscale8</scan:ColorMode></scan:ColorModes>{}",
        resolutions(&[(600, 600), (300, 600), (150, 150)])
    );
    let second = format!("{GREY_AND_COLOUR}{}", resolutions(&[(150, 150), (75, 75)]));
    let platen = format!(
        "{LIMITS}<scan:SettingProfiles>\
         <scan:SettingProfile>{first}</scan:SettingProfile>\
         <scan:SettingProfile>{second}</scan:SettingProfile>\
         </scan:SettingProfiles>"
    );
    let parsed = parse_capabilities(caps_document(&platen).as_bytes()).unwrap();
    assert_eq!(parsed.resolutions, [75, 150, 600]);
    assert_eq!(parsed.color_modes, [ColorMode::Grey, ColorMode::Colour]);
}

#[test]
fn ignores_capabilities_outside_the_platen() {
    let adf = format!(
        "<scan:Adf><scan:AdfSimplexInputCaps>{}<scan:SettingProfiles><scan:SettingProfile>\
         <scan:ColorModes><scan:ColorMode>RGB24</scan:ColorMode></scan:ColorModes>{}\
         </scan:SettingProfile></scan:SettingProfiles></scan:AdfSimplexInputCaps></scan:Adf>",
        "<scan:MaxWidth>9</scan:MaxWidth><scan:MaxHeight>9</scan:MaxHeight>",
        resolutions(&[(1200, 1200)])
    );
    let platen = format!(
        "{LIMITS}<scan:SettingProfiles><scan:SettingProfile>\
         <scan:ColorModes><scan:ColorMode>Grayscale8</scan:ColorMode></scan:ColorModes>{}\
         </scan:SettingProfile></scan:SettingProfiles>",
        resolutions(&[(75, 75)])
    );
    let xml = caps_document(&platen).replace("<scan:Platen>", &format!("{adf}<scan:Platen>"));
    let parsed = parse_capabilities(xml.as_bytes()).unwrap();
    assert_eq!((parsed.max_width, parsed.max_height), (2550, 3508));
    assert_eq!(parsed.resolutions, [75]);
    assert_eq!(parsed.color_modes, [ColorMode::Grey]);
}

#[test]
fn capabilities_without_a_required_element_are_an_error() {
    let missing_width = caps_with("<scan:MaxHeight>3508</scan:MaxHeight>", GREY_AND_COLOUR);
    assert_eq!(
        missing_width,
        Err(EsclError::Missing(
            "ScannerCapabilities/Platen/PlatenInputCaps/MaxWidth"
        ))
    );
    assert_eq!(
        missing_width.unwrap_err().to_string(),
        "the printer's answer has no \
         ScannerCapabilities/Platen/PlatenInputCaps/MaxWidth element"
    );
    assert_eq!(
        caps_with("<scan:MaxWidth>2550</scan:MaxWidth>", GREY_AND_COLOUR),
        Err(EsclError::Missing(
            "ScannerCapabilities/Platen/PlatenInputCaps/MaxHeight"
        ))
    );
    let no_model = caps_document(LIMITS).replace("<pwg:MakeAndModel>Test</pwg:MakeAndModel>", "");
    assert_eq!(
        parse_capabilities(no_model.as_bytes()),
        Err(EsclError::Missing("ScannerCapabilities/MakeAndModel"))
    );
    let no_platen = "<scan:ScannerCapabilities xmlns:scan=\"s\" xmlns:pwg=\"p\">\
                     <pwg:MakeAndModel>Test</pwg:MakeAndModel></scan:ScannerCapabilities>";
    assert_eq!(
        parse_capabilities(no_platen.as_bytes()),
        Err(EsclError::Missing(
            "ScannerCapabilities/Platen/PlatenInputCaps"
        ))
    );
}

#[test]
fn a_resolution_pair_without_both_values_is_an_error() {
    let no_y = "<scan:SupportedResolutions><scan:DiscreteResolutions><scan:DiscreteResolution>\
                <scan:XResolution>300</scan:XResolution>\
                </scan:DiscreteResolution></scan:DiscreteResolutions></scan:SupportedResolutions>";
    assert_eq!(
        caps_with(LIMITS, no_y),
        Err(EsclError::Missing(concat!(
            "ScannerCapabilities/Platen/PlatenInputCaps/SettingProfiles/SettingProfile/",
            "SupportedResolutions/DiscreteResolutions/DiscreteResolution/YResolution"
        )))
    );
    let no_x = no_y.replace("XResolution>300</scan:X", "YResolution>300</scan:Y");
    assert_eq!(
        caps_with(LIMITS, &no_x),
        Err(EsclError::Missing(concat!(
            "ScannerCapabilities/Platen/PlatenInputCaps/SettingProfiles/SettingProfile/",
            "SupportedResolutions/DiscreteResolutions/DiscreteResolution/XResolution"
        )))
    );
}

#[test]
fn numbers_that_do_not_parse_are_an_error() {
    let error = caps_with(
        "<scan:MaxWidth>wide</scan:MaxWidth><scan:MaxHeight>3508</scan:MaxHeight>",
        GREY_AND_COLOUR,
    )
    .unwrap_err();
    assert_eq!(
        error,
        EsclError::BadNumber {
            path: "ScannerCapabilities/Platen/PlatenInputCaps/MaxWidth",
            value: "wide".to_owned(),
        }
    );
    assert_eq!(
        error.to_string(),
        "the printer's answer has \"wide\" in ScannerCapabilities/Platen/PlatenInputCaps/MaxWidth \
         where a number belongs"
    );
    for (width, height) in [("-1", "3508"), ("2550", "4294967296"), ("", "3508")] {
        let limits = format!(
            "<scan:MaxWidth>{width}</scan:MaxWidth><scan:MaxHeight>{height}</scan:MaxHeight>"
        );
        assert!(matches!(
            caps_with(&limits, GREY_AND_COLOUR),
            Err(EsclError::BadNumber { .. })
        ));
    }
    assert!(matches!(
        caps_with(LIMITS, &resolutions(&[(75, 70_000)])),
        Err(EsclError::BadNumber {
            path: concat!(
                "ScannerCapabilities/Platen/PlatenInputCaps/SettingProfiles/SettingProfile/",
                "SupportedResolutions/DiscreteResolutions/DiscreteResolution/YResolution"
            ),
            ..
        })
    ));
}

#[test]
fn a_document_with_another_root_is_an_error() {
    let status = fixture("m130a-scanner-status-idle.xml").unwrap();
    let error = parse_capabilities(&status).unwrap_err();
    assert_eq!(
        error,
        EsclError::UnexpectedRoot {
            expected: "ScannerCapabilities",
            found: "ScannerStatus".to_owned(),
        }
    );
    assert_eq!(
        error.to_string(),
        "the printer answered with ScannerStatus where ScannerCapabilities was expected"
    );
    let caps = fixture("m130a-scanner-capabilities.xml").unwrap();
    assert!(matches!(
        parse_status(&caps),
        Err(EsclError::UnexpectedRoot {
            expected: "ScannerStatus",
            ..
        })
    ));
}

#[test]
fn malformed_xml_is_an_error() {
    let caps = fixture("m130a-scanner-capabilities.xml").unwrap();
    let cases: [&[u8]; 11] = [
        &caps[..caps.len() / 2],
        b"<a><b></b>",
        b"<a>&#0;</a>",
        b"",
        b"<!-- only a comment -->",
        b"<a><b></a></b>",
        b"<a></a><b></b>",
        b"<a></a>text",
        b"<a>&nbsp;</a>",
        b"<a>\xff</a>",
        b"</a>",
    ];
    for xml in cases {
        let error = parse_capabilities(xml).unwrap_err();
        assert!(
            matches!(error, EsclError::Xml(_)),
            "{:?} gave {error:?}",
            String::from_utf8_lossy(xml)
        );
        assert!(
            error
                .to_string()
                .starts_with("the printer sent malformed XML: "),
            "{error}"
        );
    }
}

#[test]
fn resolves_entities_character_references_and_cdata() {
    let platen = format!(
        "{LIMITS}<scan:SettingProfiles><scan:SettingProfile>{GREY_AND_COLOUR}\
         </scan:SettingProfile></scan:SettingProfiles>"
    );
    let xml = caps_document(&platen).replace(
        "<pwg:MakeAndModel>Test</pwg:MakeAndModel>",
        "<pwg:MakeAndModel>HP &amp; Co &lt;&#x41;&#66;&gt; <![CDATA[<raw>]]></pwg:MakeAndModel>",
    );
    assert_eq!(
        parse_capabilities(xml.as_bytes()).unwrap().make_and_model,
        "HP & Co <AB> <raw>"
    );
}

#[test]
fn self_closing_elements_count_as_empty() {
    let platen = format!(
        "{LIMITS}<scan:SettingProfiles><scan:SettingProfile>{GREY_AND_COLOUR}\
         </scan:SettingProfile></scan:SettingProfiles>"
    );
    let xml = caps_document(&platen).replace(
        "<pwg:MakeAndModel>Test</pwg:MakeAndModel>",
        "<pwg:MakeAndModel/>",
    );
    assert_eq!(
        parse_capabilities(xml.as_bytes()).unwrap().make_and_model,
        ""
    );
}

#[test]
fn parses_the_idle_status() {
    assert_eq!(
        status_fixture("idle"),
        ScannerStatus {
            state: ScannerState::Idle,
            jobs: vec![JobInfo {
                uri: "/eSCL/ScanJobs/1001".to_owned(),
                state: "Completed".to_owned(),
                images_completed: 0,
            }],
        }
    );
}

#[test]
fn parses_the_processing_status_without_jobs() {
    assert_eq!(
        status_fixture("processing"),
        ScannerStatus {
            state: ScannerState::Processing,
            jobs: Vec::new(),
        }
    );
}

#[test]
fn parses_the_completed_status_with_newest_job_first() {
    let status = status_fixture("completed");
    assert_eq!(status.state, ScannerState::Idle);
    assert_eq!(
        status.jobs,
        [
            JobInfo {
                uri: "/eSCL/ScanJobs/1002".to_owned(),
                state: "Completed".to_owned(),
                images_completed: 1,
            },
            JobInfo {
                uri: "/eSCL/ScanJobs/1001".to_owned(),
                state: "Completed".to_owned(),
                images_completed: 0,
            },
        ]
    );
}

fn status_with(body: &str) -> Result<ScannerStatus, EsclError> {
    let xml =
        format!("<scan:ScannerStatus xmlns:scan=\"s\" xmlns:pwg=\"p\">{body}</scan:ScannerStatus>");
    parse_status(xml.as_bytes())
}

#[test]
fn maps_every_scanner_state() {
    let cases = [
        ("Idle", ScannerState::Idle),
        ("Processing", ScannerState::Processing),
        ("Testing", ScannerState::Testing),
        ("Stopped", ScannerState::Stopped),
        ("Down", ScannerState::Down),
        ("idle", ScannerState::Other("idle".to_owned())),
        ("Warming", ScannerState::Other("Warming".to_owned())),
    ];
    for (text, state) in cases {
        let status = status_with(&format!("<pwg:State> {text} </pwg:State>")).unwrap();
        assert_eq!(status.state, state);
    }
}

#[test]
fn status_without_a_state_is_an_error() {
    assert_eq!(
        status_with("<scan:Jobs/>"),
        Err(EsclError::Missing("ScannerStatus/State"))
    );
}

#[test]
fn job_info_needs_uri_state_and_image_count() {
    let job = |inner: &str| {
        status_with(&format!(
            "<pwg:State>Idle</pwg:State><scan:Jobs><scan:JobInfo>{inner}</scan:JobInfo></scan:Jobs>"
        ))
    };
    let uri = "<pwg:JobUri>/eSCL/ScanJobs/7</pwg:JobUri>";
    let state = "<pwg:JobState>Processing</pwg:JobState>";
    let images = "<pwg:ImagesCompleted>3</pwg:ImagesCompleted>";
    assert_eq!(
        job(&format!("{state}{images}{uri}")).unwrap().jobs,
        [JobInfo {
            uri: "/eSCL/ScanJobs/7".to_owned(),
            state: "Processing".to_owned(),
            images_completed: 3,
        }]
    );
    assert_eq!(
        job(&format!("{state}{images}")),
        Err(EsclError::Missing("ScannerStatus/Jobs/JobInfo/JobUri"))
    );
    assert_eq!(
        job(&format!("{uri}{images}")),
        Err(EsclError::Missing("ScannerStatus/Jobs/JobInfo/JobState"))
    );
    assert_eq!(
        job(&format!("{uri}{state}")),
        Err(EsclError::Missing(
            "ScannerStatus/Jobs/JobInfo/ImagesCompleted"
        ))
    );
    assert!(matches!(
        job(&format!(
            "{uri}{state}<pwg:ImagesCompleted>x</pwg:ImagesCompleted>"
        )),
        Err(EsclError::BadNumber {
            path: "ScannerStatus/Jobs/JobInfo/ImagesCompleted",
            ..
        })
    ));
}

#[test]
fn job_path_keeps_a_relative_location() {
    assert_eq!(
        job_path("/eSCL/ScanJobs/1002").as_deref(),
        Ok("/eSCL/ScanJobs/1002")
    );
}

#[test]
fn job_path_takes_the_path_of_an_absolute_location() {
    for location in [
        "http://localhost/eSCL/ScanJobs/1002",
        "http://localhost:60000/eSCL/ScanJobs/1002",
        "http://localhost:80/eSCL/ScanJobs/1002/",
        "https://printer.local:443/eSCL/ScanJobs/1002",
        "HTTP://LOCALHOST/eSCL/ScanJobs/1002",
        "Https://localhost/eSCL/ScanJobs/1002",
        "  /eSCL/ScanJobs/1002  ",
    ] {
        assert_eq!(
            job_path(location).as_deref(),
            Ok("/eSCL/ScanJobs/1002"),
            "{location}"
        );
    }
    assert_eq!(
        job_path("http://localhost/eSCL/ScanJobs/urn:uuid:4509a320-00a0-008f-00b6-0026").as_deref(),
        Ok("/eSCL/ScanJobs/urn:uuid:4509a320-00a0-008f-00b6-0026")
    );
}

#[test]
fn job_path_rejects_locations_that_are_not_scan_jobs() {
    for location in [
        "",
        "/",
        "/eSCL/ScanJobs/",
        "/eSCL/ScanJobs",
        "/eSCL/ScannerStatus",
        "eSCL/ScanJobs/1002",
        "/other/eSCL/ScanJobs/1002",
        "/eSCL/ScanJobs/1002/NextDocument",
        "/eSCL/ScanJobs/10 02",
        "/eSCL/ScanJobs/1002\r\nX-Injected: 1",
        "/eSCL/ScanJobs/1002?x=1",
        "/eSCL/ScanJobs/1002#x",
        "http://localhost",
        "http://localhost/eSCL/ScannerStatus",
        "ftp://localhost/eSCL/ScanJobs/1002",
        "localhost/eSCL/ScanJobs/1002",
    ] {
        let error = job_path(location).unwrap_err();
        assert_eq!(error, EsclError::BadJobLocation(location.to_owned()));
    }
    assert_eq!(
        job_path("/x").unwrap_err().to_string(),
        "the printer gave the scan job an unusable location \"/x\""
    );
}

fn nested(depth: usize) -> Vec<u8> {
    format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth)).into_bytes()
}

#[test]
fn accepts_elements_nested_32_deep() {
    assert_eq!(
        parse_capabilities(&nested(32)),
        Err(EsclError::UnexpectedRoot {
            expected: "ScannerCapabilities",
            found: "a".to_owned(),
        })
    );
}

#[test]
fn rejects_elements_nested_deeper_than_32() {
    let too_deep = EsclError::Xml("elements nested deeper than 32".to_owned());
    assert_eq!(parse_status(&nested(100_000)), Err(too_deep.clone()));
    assert_eq!(parse_capabilities(&nested(33)), Err(too_deep));
}

#[test]
fn rejects_documents_over_1_mib() {
    let padded = |size: usize| {
        let mut xml = b"<a>".to_vec();
        xml.resize(size - 4, b' ');
        xml.extend_from_slice(b"</a>");
        xml
    };
    assert!(matches!(
        parse_status(&padded(1 << 20)),
        Err(EsclError::UnexpectedRoot { .. })
    ));
    assert_eq!(
        parse_status(&padded((1 << 20) + 1)),
        Err(EsclError::Xml(
            "the document is larger than 1 MiB".to_owned()
        ))
    );
}
