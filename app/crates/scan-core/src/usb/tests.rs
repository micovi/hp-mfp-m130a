use std::collections::VecDeque;

use super::*;

/// What a [`FakeIn`] does next.
#[derive(Debug)]
enum Step {
    /// Completes one transfer like this.
    Complete(Received),
    /// Completes transfers empty for this long, as the idle printer does between bursts.
    Gap(Duration),
}

/// A bulk IN endpoint that plays back its steps, then answers zero-length packets forever,
/// as the printer's idle endpoint does.
#[derive(Default)]
struct FakeIn {
    steps: VecDeque<Step>,
    gap_end: Option<Instant>,
    timeouts: Vec<Duration>,
}

impl FakeIn {
    fn new(completions: impl IntoIterator<Item = Received>) -> Self {
        Self::steps(completions.into_iter().map(Step::Complete))
    }

    fn steps(steps: impl IntoIterator<Item = Step>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            ..Self::default()
        }
    }
}

impl BulkIn for FakeIn {
    fn receive(&mut self, timeout: Duration) -> Received {
        self.timeouts.push(timeout);
        while let Some(Step::Gap(gap)) = self.steps.front() {
            let end = *self.gap_end.get_or_insert(Instant::now() + *gap);
            if Instant::now() < end {
                return empty();
            }
            self.steps.pop_front();
            self.gap_end = None;
        }
        match self.steps.pop_front() {
            Some(Step::Complete(received)) => received,
            Some(Step::Gap(_)) | None => empty(),
        }
    }
}

/// A bulk IN endpoint that never stops sending.
struct Endless;

impl BulkIn for Endless {
    fn receive(&mut self, _timeout: Duration) -> Received {
        data(b"jpeg")
    }
}

fn data(bytes: &[u8]) -> Received {
    Received {
        data: bytes.to_vec(),
        status: Ok(()),
    }
}

fn empty() -> Received {
    data(b"")
}

fn failed(bytes: &[u8], error: TransferError) -> Received {
    Received {
        data: bytes.to_vec(),
        status: Err(error),
    }
}

fn soon() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

#[test]
fn read_skips_empty_completions_and_returns_the_first_data() {
    let mut input = FakeIn::new([empty(), empty(), data(b"HTTP/1.1"), data(b" 200")]);
    assert_eq!(read_next(&mut input, soon()), Ok(b"HTTP/1.1".to_vec()));
    assert_eq!(input.timeouts.len(), 3);
    assert_eq!(read_next(&mut input, soon()), Ok(b" 200".to_vec()));
}

#[test]
fn read_never_waits_past_the_deadline() {
    let mut input = FakeIn::new([data(b"x")]);
    let deadline = soon();
    read_next(&mut input, deadline).unwrap();
    assert!(input.timeouts[0] <= Duration::from_secs(5));
    assert!(input.timeouts[0] > Duration::from_secs(4));
}

#[test]
fn read_times_out_without_a_transfer_once_the_deadline_has_passed() {
    let mut input = FakeIn::new([data(b"x")]);
    assert_eq!(
        read_next(&mut input, Instant::now()),
        Err(TransportError::Timeout)
    );
    assert_eq!(input.timeouts, []);
}

#[test]
fn read_times_out_when_only_empty_packets_arrive_and_pauses_between_them() {
    let mut input = FakeIn::default();
    let started = Instant::now();
    let wait = Duration::from_millis(30);
    assert_eq!(
        read_next(&mut input, started + wait),
        Err(TransportError::Timeout)
    );
    assert!(started.elapsed() >= wait);
    let transfers = input.timeouts.len();
    assert!((2..=31).contains(&transfers), "{transfers} transfers");
}

#[test]
fn read_resubmits_after_a_transfer_timeout_until_the_deadline() {
    let mut input = FakeIn::new([failed(b"", TransferError::Cancelled), data(b"late")]);
    assert_eq!(read_next(&mut input, soon()), Ok(b"late".to_vec()));
}

#[test]
fn read_returns_the_data_of_a_transfer_cut_short() {
    let mut input = FakeIn::new([failed(b"part", TransferError::Cancelled)]);
    assert_eq!(read_next(&mut input, soon()), Ok(b"part".to_vec()));
    let mut input = FakeIn::new([failed(b"last", TransferError::Disconnected)]);
    assert_eq!(read_next(&mut input, soon()), Ok(b"last".to_vec()));
}

#[test]
fn read_reports_a_disconnect() {
    let mut input = FakeIn::new([empty(), failed(b"", TransferError::Disconnected)]);
    assert_eq!(
        read_next(&mut input, soon()),
        Err(TransportError::Disconnected)
    );
}

#[test]
fn read_describes_other_transfer_failures() {
    for (error, text) in [
        (TransferError::Stall, "endpoint stalled"),
        (TransferError::Fault, "hardware fault"),
        (TransferError::InvalidArgument, ""),
        (TransferError::Unknown(0xe000_02ed), ""),
    ] {
        let mut input = FakeIn::new([failed(b"", error)]);
        let result = read_next(&mut input, soon());
        assert!(
            matches!(&result, Err(TransportError::Io(message))
                if message.starts_with("reading from the printer failed: ")
                    && message.contains(text)),
            "{error:?} gave {result:?}"
        );
    }
}

const QUIET: Duration = Duration::from_millis(40);
const TEST_LIMITS: DrainLimits = DrainLimits {
    quiet: QUIET,
    longest: Duration::from_secs(2),
    most_bytes: 1024 * 1024,
};

fn gap(milliseconds: u64) -> Step {
    Step::Gap(Duration::from_millis(milliseconds))
}

fn burst(bytes: &[u8]) -> Step {
    Step::Complete(data(bytes))
}

#[test]
fn drain_of_a_clean_endpoint_is_one_read() {
    let mut input = FakeIn::new([data(b"next")]);
    input.steps.push_front(Step::Complete(empty()));
    assert_eq!(drain(&mut input, false, &TEST_LIMITS), Ok(0));
    assert_eq!(input.timeouts, [DRAIN_READ_TIMEOUT]);
    let mut input = FakeIn::new([failed(b"", TransferError::Cancelled), data(b"next")]);
    assert_eq!(drain(&mut input, false, &TEST_LIMITS), Ok(0));
    assert_eq!(input.timeouts.len(), 1);
}

#[test]
fn drain_waits_out_bursts_separated_by_short_gaps() {
    let mut input = FakeIn::steps([
        burst(b"stale"),
        gap(10),
        burst(b"jpeg"),
        burst(b"data"),
        gap(25),
        burst(b"more"),
        gap(150),
        burst(b"next"),
    ]);
    let started = Instant::now();
    assert_eq!(drain(&mut input, false, &TEST_LIMITS), Ok(17));
    assert!(started.elapsed() >= QUIET + Duration::from_millis(35));
    assert!(started.elapsed() < Duration::from_millis(150));
    assert!(input.timeouts[1..].iter().all(|&timeout| timeout <= QUIET));
    assert!(matches!(input.steps.back(), Some(Step::Complete(next)) if next.data == b"next"));
}

#[test]
fn drain_counts_partial_data_of_cut_short_transfers() {
    let mut input = FakeIn::new([
        failed(b"ab", TransferError::Cancelled),
        failed(b"c", TransferError::Cancelled),
        empty(),
    ]);
    assert_eq!(drain(&mut input, false, &TEST_LIMITS), Ok(3));
}

#[test]
fn a_dirty_drain_waits_for_quiet_even_when_the_first_read_is_empty() {
    let mut input = FakeIn::steps([gap(15), burst(b"late")]);
    let started = Instant::now();
    assert_eq!(drain(&mut input, true, &TEST_LIMITS), Ok(4));
    assert!(started.elapsed() >= QUIET + Duration::from_millis(15));

    let mut input = FakeIn::default();
    let started = Instant::now();
    assert_eq!(drain(&mut input, true, &TEST_LIMITS), Ok(0));
    assert!(started.elapsed() >= QUIET);
    assert!(input.timeouts.len() > 1);
}

#[test]
fn drain_gives_up_on_a_printer_that_keeps_sending() {
    let limits = DrainLimits {
        longest: Duration::from_millis(50),
        most_bytes: usize::MAX,
        ..TEST_LIMITS
    };
    let started = Instant::now();
    let result = drain(&mut Endless, false, &limits);
    assert!(started.elapsed() >= limits.longest);
    assert!(
        matches!(&result, Err(TransportError::Io(message)) if message.contains("keeps sending")),
        "{result:?}"
    );

    let limits = DrainLimits {
        most_bytes: 10,
        ..TEST_LIMITS
    };
    let started = Instant::now();
    assert!(drain(&mut Endless, true, &limits).is_err());
    assert!(started.elapsed() < limits.longest);
}

#[test]
fn drain_reports_failures() {
    let mut input = FakeIn::new([failed(b"", TransferError::Disconnected)]);
    assert_eq!(
        drain(&mut input, false, &TEST_LIMITS),
        Err(TransportError::Disconnected)
    );
    let mut input = FakeIn::new([data(b"ab"), failed(b"", TransferError::Disconnected)]);
    assert_eq!(
        drain(&mut input, false, &TEST_LIMITS),
        Err(TransportError::Disconnected)
    );
    let mut input = FakeIn::new([failed(b"", TransferError::Stall)]);
    assert!(matches!(
        drain(&mut input, true, &TEST_LIMITS),
        Err(TransportError::Io(message)) if message.starts_with("draining")
    ));
}

#[test]
fn production_drain_limits() {
    assert_eq!(DRAIN_LIMITS.quiet, Duration::from_millis(500));
    assert_eq!(DRAIN_LIMITS.longest, Duration::from_secs(15));
    assert_eq!(DRAIN_LIMITS.most_bytes, 256 * 1024 * 1024);
}

/// A bulk OUT endpoint that records each transfer and fails the ones it is told to.
struct FakeOut {
    max_packet_size: usize,
    sent: Vec<Vec<u8>>,
    timeouts: Vec<Duration>,
    failures: VecDeque<Result<(), TransferError>>,
}

impl FakeOut {
    fn new(max_packet_size: usize) -> Self {
        Self {
            max_packet_size,
            sent: Vec::new(),
            timeouts: Vec::new(),
            failures: VecDeque::new(),
        }
    }
}

impl BulkOut for FakeOut {
    fn max_packet_size(&self) -> usize {
        self.max_packet_size
    }

    fn send(&mut self, data: &[u8], timeout: Duration) -> Result<(), TransferError> {
        self.timeouts.push(timeout);
        self.sent.push(data.to_vec());
        self.failures.pop_front().unwrap_or(Ok(()))
    }
}

#[test]
fn write_adds_a_zero_length_packet_only_after_a_full_last_packet() {
    for (len, packet, transfers) in [
        (1, 512, vec![1]),
        (511, 512, vec![511]),
        (512, 512, vec![512, 0]),
        (513, 512, vec![513]),
        (1024, 512, vec![1024, 0]),
        (128, 64, vec![128, 0]),
        (130, 64, vec![130]),
    ] {
        let mut output = FakeOut::new(packet);
        write_message(&mut output, &vec![7; len]).unwrap();
        let sizes: Vec<usize> = output.sent.iter().map(Vec::len).collect();
        assert_eq!(sizes, transfers, "{len} bytes in {packet}-byte packets");
        assert!(
            output
                .timeouts
                .iter()
                .all(|&timeout| timeout == WRITE_TIMEOUT)
        );
    }
}

#[test]
fn write_of_nothing_sends_nothing() {
    let mut output = FakeOut::new(512);
    assert_eq!(write_message(&mut output, b""), Ok(()));
    assert_eq!(output.sent, Vec::<Vec<u8>>::new());
}

#[test]
fn zero_length_packet_rule() {
    assert!(needs_zero_length_packet(512, 512));
    assert!(!needs_zero_length_packet(500, 512));
    assert!(needs_zero_length_packet(0, 64));
    assert!(!needs_zero_length_packet(64, 0));
}

#[test]
fn write_reports_failures_and_stops() {
    let mut output = FakeOut::new(4);
    output.failures.push_back(Err(TransferError::Cancelled));
    assert_eq!(
        write_message(&mut output, b"abcd"),
        Err(TransportError::Timeout)
    );
    assert_eq!(output.sent.len(), 1);

    let mut output = FakeOut::new(4);
    output
        .failures
        .extend([Ok(()), Err(TransferError::Disconnected)]);
    assert_eq!(
        write_message(&mut output, b"abcd"),
        Err(TransportError::Disconnected)
    );
    assert_eq!(output.sent, [b"abcd".to_vec(), Vec::new()]);

    let mut output = FakeOut::new(4);
    output.failures.push_back(Err(TransferError::Stall));
    assert!(matches!(
        write_message(&mut output, b"abc"),
        Err(TransportError::Io(message)) if message.starts_with("writing to the printer failed")
    ));
}

/// An endpoint descriptor's address, attributes and max packet size.
type EndpointFields = (u8, u8, u16);
/// An interface descriptor's number, alternate setting, class triple and endpoints.
type InterfaceFields<'a> = (u8, u8, (u8, u8, u8), &'a [EndpointFields]);

/// Builds a configuration descriptor holding these interfaces.
fn configuration(interfaces: &[InterfaceFields<'_>]) -> Vec<u8> {
    let mut bytes = vec![9, 2, 0, 0, 0, 1, 0, 0xC0, 1];
    let mut count = 0;
    for &(number, alternate, (class, subclass, protocol), endpoints) in interfaces {
        if alternate == 0 {
            count += 1;
        }
        let endpoint_count = u8::try_from(endpoints.len()).unwrap();
        bytes.extend([
            9,
            4,
            number,
            alternate,
            endpoint_count,
            class,
            subclass,
            protocol,
            0,
        ]);
        for &(address, attributes, size) in endpoints {
            let [low, high] = size.to_le_bytes();
            bytes.extend([7, 5, address, attributes, low, high, 0]);
        }
    }
    let [low, high] = u16::try_from(bytes.len()).unwrap().to_le_bytes();
    bytes[2] = low;
    bytes[3] = high;
    bytes[4] = count;
    bytes
}

const BULK: u8 = 2;
const INTERRUPT: u8 = 3;

/// The M130a's interfaces, as `just probe` lists them, at high speed.
fn m130a() -> Vec<u8> {
    configuration(&[
        (0, 0, (7, 1, 2), &[(0x01, BULK, 512), (0x81, BULK, 512)]),
        (1, 0, (0xFF, 4, 1), &[(0x07, BULK, 512), (0x87, BULK, 512)]),
        (2, 0, (0xFF, 4, 1), &[(0x09, BULK, 512), (0x89, BULK, 512)]),
        (3, 0, (0xFF, 9, 1), &[(0x0B, BULK, 512), (0x8B, BULK, 512)]),
        (4, 0, (0xFF, 9, 1), &[(0x0C, BULK, 512), (0x8C, BULK, 512)]),
        (5, 0, (0xFF, 2, 1), &[(0x03, BULK, 512), (0x83, BULK, 512)]),
    ])
}

fn found(descriptor: &[u8]) -> Vec<IppInterface> {
    ipp_interfaces(&ConfigurationDescriptor::new(descriptor).unwrap())
}

#[test]
fn finds_the_m130a_ipp_interfaces_in_order() {
    assert_eq!(
        found(&m130a()),
        [
            IppInterface {
                number: 3,
                bulk_out: 0x0B,
                bulk_in: 0x8B,
            },
            IppInterface {
                number: 4,
                bulk_out: 0x0C,
                bulk_in: 0x8C,
            },
        ]
    );
}

#[test]
fn ipp_interfaces_are_sorted_by_number() {
    let descriptor = configuration(&[
        (4, 0, IPP_OVER_USB, &[(0x0C, BULK, 64), (0x8C, BULK, 64)]),
        (3, 0, IPP_OVER_USB, &[(0x8B, BULK, 64), (0x0B, BULK, 64)]),
    ]);
    let numbers: Vec<u8> = found(&descriptor).iter().map(|ipp| ipp.number).collect();
    assert_eq!(numbers, [3, 4]);
    assert_eq!(found(&descriptor)[0].bulk_out, 0x0B);
}

#[test]
fn ignores_other_alternate_settings_and_interfaces_without_a_bulk_pair() {
    let descriptor = configuration(&[
        (0, 0, (7, 1, 2), &[(0x01, BULK, 512), (0x81, BULK, 512)]),
        (0, 1, IPP_OVER_USB, &[(0x01, BULK, 512), (0x81, BULK, 512)]),
        (1, 0, IPP_OVER_USB, &[(0x02, BULK, 512)]),
        (
            2,
            0,
            IPP_OVER_USB,
            &[(0x03, BULK, 512), (0x83, INTERRUPT, 8)],
        ),
        (3, 0, IPP_OVER_USB, &[]),
        (
            4,
            0,
            IPP_OVER_USB,
            &[(0x85, INTERRUPT, 8), (0x04, BULK, 512), (0x84, BULK, 512)],
        ),
    ]);
    assert_eq!(
        found(&descriptor),
        [IppInterface {
            number: 4,
            bulk_out: 0x04,
            bulk_in: 0x84,
        }]
    );
}

#[test]
fn a_device_without_ipp_interfaces_has_none() {
    let descriptor = configuration(&[(0, 0, (7, 1, 2), &[(0x01, BULK, 64), (0x81, BULK, 64)])]);
    assert_eq!(found(&descriptor), Vec::<IppInterface>::new());
}

fn failure(kind: ErrorKind) -> OpenFailure {
    OpenFailure {
        kind,
        detail: format!("claiming IF3: {kind:?}"),
    }
}

#[test]
fn nothing_to_claim_is_not_connected() {
    assert_eq!(open_error(&[]), ScanError::NotConnected);
    let gone = [
        failure(ErrorKind::Disconnected),
        failure(ErrorKind::Disconnected),
    ];
    assert_eq!(open_error(&gone), ScanError::NotConnected);
}

#[test]
fn a_busy_interface_means_another_app_holds_it() {
    let busy = [failure(ErrorKind::Busy), failure(ErrorKind::Busy)];
    assert_eq!(open_error(&busy), ScanError::InterfaceBusy);
    let mixed = [failure(ErrorKind::Other), failure(ErrorKind::Busy)];
    assert_eq!(open_error(&mixed), ScanError::InterfaceBusy);
}

#[test]
fn other_open_failures_are_described() {
    let failures = [
        OpenFailure {
            kind: ErrorKind::PermissionDenied,
            detail: "claiming IF3: access denied".to_owned(),
        },
        failure(ErrorKind::Disconnected),
    ];
    assert_eq!(
        open_error(&failures),
        ScanError::Protocol("claiming IF3: access denied; claiming IF3: Disconnected".to_owned())
    );
}

#[test]
fn the_transport_can_move_to_another_thread() {
    fn assert_send<T: Send>() {}
    assert_send::<UsbTransport>();
    assert_send::<Scanner<UsbTransport>>();
}

/// Runs [`first_started`] over interfaces 3 and 4: `claims` and `starts` say how each fares.
/// Returns the result and the log of what happened, in order.
fn open_with(
    claims: [Result<(), ErrorKind>; 2],
    starts: [Result<(), ScanError>; 2],
) -> (Result<u8, ScanError>, Vec<String>) {
    let mut starts = starts.map(Some);
    let log = std::cell::RefCell::new(Vec::new());
    let result = first_started(
        vec![3_u8, 4],
        Vec::new(),
        |number| {
            log.borrow_mut().push(format!("claim {number}"));
            claims[usize::from(number - 3)]
                .map(|()| number)
                .map_err(failure)
        },
        |number| {
            log.borrow_mut().push(format!("start {number}"));
            let start = starts[usize::from(number - 3)].take();
            start.unwrap_or(Ok(())).map(|()| number)
        },
    );
    (result, log.into_inner())
}

fn timeout() -> ScanError {
    ScanError::Timeout {
        waiting_for: "the scanner's capabilities".to_owned(),
    }
}

#[test]
fn opens_the_first_interface_that_answers() {
    let (result, log) = open_with([Ok(()), Ok(())], [Ok(()), Ok(())]);
    assert_eq!(result, Ok(3));
    assert_eq!(log, ["claim 3", "start 3"]);
}

#[test]
fn falls_back_to_the_next_interface_when_one_is_stuck_or_taken() {
    let (result, log) = open_with([Ok(()), Ok(())], [Err(timeout()), Ok(())]);
    assert_eq!(result, Ok(4));
    assert_eq!(log, ["claim 3", "start 3", "claim 4", "start 4"]);

    let (result, log) = open_with([Err(ErrorKind::Busy), Ok(())], [Ok(()), Ok(())]);
    assert_eq!(result, Ok(4));
    assert_eq!(log, ["claim 3", "claim 4", "start 4"]);
}

#[test]
fn the_last_start_error_wins_once_anything_was_claimed() {
    let (result, _) = open_with(
        [Ok(()), Ok(())],
        [Err(timeout()), Err(ScanError::Disconnected)],
    );
    assert_eq!(result, Err(ScanError::Disconnected));
    let (result, _) = open_with([Ok(()), Err(ErrorKind::Busy)], [Err(timeout()), Ok(())]);
    assert_eq!(result, Err(timeout()));
}

#[test]
fn nothing_claimed_is_an_open_error() {
    let busy = [Err(ErrorKind::Busy), Err(ErrorKind::Busy)];
    let (result, log) = open_with(busy, [Ok(()), Ok(())]);
    assert_eq!(result, Err(ScanError::InterfaceBusy));
    assert_eq!(log, ["claim 3", "claim 4"]);

    let none: Result<u8, ScanError> = first_started(
        Vec::<u8>::new(),
        vec![failure(ErrorKind::Disconnected)],
        Ok,
        Ok,
    );
    assert_eq!(none, Err(ScanError::NotConnected));
}
