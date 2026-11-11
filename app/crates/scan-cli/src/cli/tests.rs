use clap::CommandFactory;
use clap::error::ErrorKind;

use super::*;

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("m130a-scan").chain(args.iter().copied()))
}

fn scan_args(args: &[&str]) -> ScanArgs {
    match parse(args).unwrap().command {
        Command::Scan(scan) => scan,
        Command::Status => unreachable!("parsed as status: {args:?}"),
    }
}

#[test]
fn the_definition_is_consistent() {
    Cli::command().debug_assert();
}

#[test]
fn status_takes_no_options() {
    assert_eq!(parse(&["status"]).unwrap().command, Command::Status);
    assert!(parse(&["status", "--grey"]).is_err());
}

#[test]
fn scan_defaults_to_colour_at_150_dpi() {
    let args = scan_args(&["scan", "--out", "a.pdf"]);
    assert_eq!(args.out, PathBuf::from("a.pdf"));
    assert!(!args.force);
    let expected = ScanSettings {
        color: ColorMode::Colour,
        dpi: 150,
    };
    assert_eq!(args.settings(), expected);
}

#[test]
fn scan_takes_grey_resolution_and_force() {
    for dpi in ["150", "300", "600"] {
        let args = scan_args(&["scan", "--grey", "--dpi", dpi, "--force", "--out", "b.pdf"]);
        assert!(args.force);
        let expected = ScanSettings {
            color: ColorMode::Grey,
            dpi: dpi.parse().unwrap(),
        };
        assert_eq!(args.settings(), expected);
    }
}

#[test]
fn scan_offers_only_150_300_and_600_dpi() {
    for dpi in ["75", "200", "1200", "0", "-300", "300dpi", ""] {
        let error = parse(&["scan", "--out", "a.pdf", &format!("--dpi={dpi}")]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidValue, "{dpi:?}");
        let message = error.to_string();
        assert!(
            message.contains("[possible values: 150, 300, 600]"),
            "{message}"
        );
    }
}

#[test]
fn scan_needs_an_output_path() {
    let error = parse(&["scan", "--grey"]).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    assert!(parse(&[]).is_err());
}

#[test]
fn usage_errors_exit_1_and_help_and_version_exit_0() {
    let usage = parse(&["scan"]).unwrap_err();
    assert_eq!(exit_code(&usage), ExitCode::FAILURE);
    let help = parse(&["--help"]).unwrap_err();
    assert_eq!(help.kind(), ErrorKind::DisplayHelp);
    assert_eq!(exit_code(&help), ExitCode::SUCCESS);
    let version = parse(&["--version"]).unwrap_err();
    assert_eq!(exit_code(&version), ExitCode::SUCCESS);
}
