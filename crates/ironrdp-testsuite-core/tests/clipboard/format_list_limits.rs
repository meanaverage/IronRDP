//! What a Format List PDU may carry: at most [`MAX_FORMAT_COUNT`] formats, with names of at most
//! [`MAX_FORMAT_NAME_LENGTH`] characters.
//! A list over these limits is answered with CB_RESPONSE_FAIL, and the channel goes on.

use std::sync::{Arc, Mutex};

use ironrdp_cliprdr::CliprdrClient;
use ironrdp_cliprdr::backend::CliprdrBackend;
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardFormatName, ClipboardGeneralCapabilityFlags, ClipboardPdu,
    FileContentsRequest, FileContentsResponse, FormatDataRequest, FormatDataResponse, FormatList, FormatListResponse,
    LockDataId, MAX_FORMAT_COUNT, MAX_FORMAT_NAME_LENGTH,
};
use ironrdp_core::AsAny;
use ironrdp_svc::SvcProcessor as _;

use ironrdp_testsuite_core::clipboard::{init_ready_client_with_backend, real_now_ms};

fn named(id: u32, name: &str) -> ClipboardFormat {
    ClipboardFormat::new(ClipboardFormatId::new(id)).with_name(ClipboardFormatName::new(name.to_owned()))
}

fn unnamed(id: ClipboardFormatId) -> ClipboardFormat {
    ClipboardFormat::new(id)
}

/// msgFlags of a Format List PDU with Unicode names.
const UNICODE_NAMES: u16 = 0;
/// msgFlags of a Format List PDU with ANSI names (CB_ASCII_NAMES).
const ASCII_NAMES: u16 = 0x0004;

/// A Format List PDU with `flags` as its msgFlags and `body` as its format entries, as the server sends it.
fn format_list_pdu(flags: u16, body: &[u8]) -> Vec<u8> {
    let mut pdu = vec![0x02, 0x00]; // msgType CB_FORMAT_LIST
    pdu.extend_from_slice(&flags.to_le_bytes());
    pdu.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
    pdu.extend_from_slice(body);
    pdu
}

fn decode_long_names(pdu: &[u8]) -> Result<Vec<ClipboardFormat>, ironrdp_pdu::PduError> {
    match ironrdp_core::decode::<ClipboardPdu<'_>>(pdu).unwrap() {
        ClipboardPdu::FormatList(list) => list.get_formats(true),
        other => panic!("expected a Format List, got {other:?}"),
    }
}

// oracle: property round-trip - a list at the limits decodes to the formats it was encoded from.
#[test]
fn takes_a_format_list_at_the_limits() {
    let name = "n".repeat(MAX_FORMAT_NAME_LENGTH);
    let list = vec![named(0xC001, &name); MAX_FORMAT_COUNT];

    for encoded in [
        FormatList::new_unicode(&list, true).unwrap(),
        FormatList::new_ascii(&list, true).unwrap(),
    ] {
        let formats = encoded.get_formats(true).unwrap();
        assert_eq!(formats.len(), MAX_FORMAT_COUNT);
        assert!(formats.iter().all(|format| format == &list[0]));
    }

    // Short format names: 32 bytes each.
    let list = vec![unnamed(ClipboardFormatId::CF_UNICODETEXT); MAX_FORMAT_COUNT];
    let formats = FormatList::new_unicode(&list, false)
        .unwrap()
        .get_formats(false)
        .unwrap();
    assert_eq!(formats, list);
}

// oracle: mutant an ANSI name's terminator counted as two bytes, or an off-by-one in MAX_FORMAT_COUNT
#[test]
fn counts_empty_ansi_names_at_the_limit() {
    let list = vec![unnamed(ClipboardFormatId::CF_DIB); MAX_FORMAT_COUNT];
    assert_eq!(
        FormatList::new_ascii(&list, true)
            .unwrap()
            .get_formats(true)
            .unwrap()
            .len(),
        MAX_FORMAT_COUNT
    );
    let too_many = vec![unnamed(ClipboardFormatId::CF_DIB); MAX_FORMAT_COUNT + 1];
    assert!(
        FormatList::new_ascii(&too_many, true)
            .unwrap()
            .get_formats(true)
            .is_err()
    );
}

// oracle: mutant an off-by-one in MAX_FORMAT_COUNT
#[test]
fn refuses_more_formats_than_the_limit() {
    let list = vec![named(0xC001, "PNG"); MAX_FORMAT_COUNT + 1];

    assert!(FormatList::new_unicode(&list, true).unwrap().get_formats(true).is_err());
    assert!(FormatList::new_ascii(&list, true).unwrap().get_formats(true).is_err());
    assert!(
        FormatList::new_unicode(&list, false)
            .unwrap()
            .get_formats(false)
            .is_err()
    );
}

// oracle: mutant an off-by-one in MAX_FORMAT_NAME_LENGTH
#[test]
fn refuses_a_format_name_longer_than_the_limit() {
    let list = [named(0xC001, &"n".repeat(MAX_FORMAT_NAME_LENGTH + 1))];

    assert!(FormatList::new_unicode(&list, true).unwrap().get_formats(true).is_err());
    assert!(FormatList::new_ascii(&list, true).unwrap().get_formats(true).is_err());
}

// oracle: mutant an off-by-one in MAX_FORMAT_NAME_LENGTH, or a name without a terminator taken at any length
#[test]
fn refuses_an_unterminated_name_longer_than_the_limit() {
    // One format whose name runs past the limit without a terminator.
    let mut body = 0xC001u32.to_le_bytes().to_vec();
    body.extend(core::iter::repeat_n([b'n', 0], MAX_FORMAT_NAME_LENGTH + 1).flatten());

    assert!(decode_long_names(&format_list_pdu(UNICODE_NAMES, &body)).is_err());
}

// oracle: mutant a name that ends with the PDU refused or cut short
#[test]
fn takes_an_unterminated_name_within_the_limit() {
    // As before the limits: a name that ends with the PDU is read to its end.
    let mut body = 0xC001u32.to_le_bytes().to_vec();
    body.extend_from_slice(b"P\0N\0G\0");

    assert_eq!(
        decode_long_names(&format_list_pdu(UNICODE_NAMES, &body)).unwrap(),
        [named(0xC001, "PNG")]
    );
}

// oracle: mutant an off-by-one in MAX_FORMAT_NAME_LENGTH for ANSI names, or one without a terminator taken at any
// length
#[test]
fn refuses_an_unterminated_ansi_name_longer_than_the_limit() {
    let mut body = 0xC001u32.to_le_bytes().to_vec();
    body.extend(core::iter::repeat_n(b'n', MAX_FORMAT_NAME_LENGTH + 1));

    assert!(decode_long_names(&format_list_pdu(ASCII_NAMES, &body)).is_err());
}

// oracle: mutant an off-by-one in MAX_FORMAT_NAME_LENGTH for a name without a terminator
#[test]
fn takes_an_unterminated_name_of_the_limit_s_length() {
    let name = "n".repeat(MAX_FORMAT_NAME_LENGTH);

    let mut unicode = 0xC001u32.to_le_bytes().to_vec();
    unicode.extend(core::iter::repeat_n([b'n', 0], MAX_FORMAT_NAME_LENGTH).flatten());
    assert_eq!(
        decode_long_names(&format_list_pdu(UNICODE_NAMES, &unicode)).unwrap(),
        [named(0xC001, &name)]
    );

    let mut ansi = 0xC001u32.to_le_bytes().to_vec();
    ansi.extend(core::iter::repeat_n(b'n', MAX_FORMAT_NAME_LENGTH));
    assert_eq!(
        decode_long_names(&format_list_pdu(ASCII_NAMES, &ansi)).unwrap(),
        [named(0xC001, &name)]
    );
}

/// Records the formats the remote copies.
#[derive(Debug)]
struct RemoteCopies(Arc<Mutex<Vec<Vec<ClipboardFormat>>>>);

impl CliprdrBackend for RemoteCopies {
    fn temporary_directory(&self) -> &str {
        "/tmp"
    }
    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::empty()
    }
    fn on_ready(&mut self) {}
    fn on_request_format_list(&mut self) {}
    fn on_process_negotiated_capabilities(&mut self, _: ClipboardGeneralCapabilityFlags) {}
    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        self.0.lock().unwrap().push(available_formats.to_vec());
    }
    fn on_format_data_request(&mut self, _: FormatDataRequest) {}
    fn on_format_data_response(&mut self, _: FormatDataResponse<'_>) {}
    fn on_file_contents_request(&mut self, _: FileContentsRequest) {}
    fn on_file_contents_response(&mut self, _: FileContentsResponse<'_>) {}
    fn on_lock(&mut self, _: LockDataId) {}
    fn on_unlock(&mut self, _: LockDataId) {}

    fn now_ms(&self) -> u64 {
        real_now_ms()
    }
    fn elapsed_ms(&self, since: u64) -> u64 {
        self.now_ms().saturating_sub(since)
    }
}

impl AsAny for RemoteCopies {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

/// A client in the Ready state, and what its backend hears of the remote's copies.
fn ready_client() -> (CliprdrClient, Arc<Mutex<Vec<Vec<ClipboardFormat>>>>) {
    let copies = Arc::new(Mutex::new(Vec::new()));
    let cliprdr = init_ready_client_with_backend(Box::new(RemoteCopies(Arc::clone(&copies))));
    (cliprdr, copies)
}

/// Hands the client the remote's Format List PDU; returns its answers.
fn process(cliprdr: &mut CliprdrClient, pdu: &[u8]) -> Vec<FormatListResponse> {
    cliprdr
        .process(pdu)
        .unwrap()
        .into_iter()
        .map(|message| {
            let bytes = message.encode_unframed_pdu().unwrap();
            match ironrdp_core::decode::<ClipboardPdu<'_>>(&bytes).unwrap() {
                ClipboardPdu::FormatListResponse(response) => response,
                other => panic!("expected a Format List Response, got {other:?}"),
            }
        })
        .collect()
}

fn encode(list: &[ClipboardFormat]) -> Vec<u8> {
    ironrdp_core::encode_vec(&ClipboardPdu::FormatList(FormatList::new_unicode(list, true).unwrap())).unwrap()
}

#[test]
fn answers_a_format_list_over_the_limit_with_a_failure_and_goes_on() {
    let (mut cliprdr, copies) = ready_client();

    // Repeated supported formats are rejected before reaching backend selection.
    let responses = process(
        &mut cliprdr,
        &encode(&vec![unnamed(ClipboardFormatId::CF_DIB); MAX_FORMAT_COUNT + 1]),
    );

    // oracle: regression #2091 - a list of duplicate CF_DIB entries past the limit is refused, and the channel goes on.
    // oracle: spec MS-RDPECLIP §2.2.3.2 - a Format List that isn't processed is answered with CB_RESPONSE_FAIL.
    assert_eq!(responses, [FormatListResponse::Fail]);
    // The remote clipboard changed to nothing the client can take.
    // oracle: mutant the backend left with the formats of the copy before
    assert_eq!(*copies.lock().unwrap(), [Vec::<ClipboardFormat>::new()]);

    // The next copy goes through.
    let list = [unnamed(ClipboardFormatId::CF_UNICODETEXT), named(0xC001, "HTML Format")];
    assert_eq!(process(&mut cliprdr, &encode(&list)), [FormatListResponse::Ok]);
    assert_eq!(copies.lock().unwrap().last().unwrap(), &list);
}

#[test]
fn answers_a_format_name_over_the_limit_with_a_failure() {
    let (mut cliprdr, copies) = ready_client();

    let list = [named(0xC001, &"n".repeat(MAX_FORMAT_NAME_LENGTH + 1))];

    // oracle: spec MS-RDPECLIP §2.2.3.2 - a Format List that isn't processed is answered with CB_RESPONSE_FAIL.
    assert_eq!(process(&mut cliprdr, &encode(&list)), [FormatListResponse::Fail]);
    // oracle: mutant the backend left with the formats of the copy before
    assert_eq!(*copies.lock().unwrap(), [Vec::<ClipboardFormat>::new()]);
}

#[test]
fn answers_a_format_list_it_cannot_decode_with_a_failure() {
    let (mut cliprdr, copies) = ready_client();

    // A name with an unpaired surrogate.
    let mut body = 0xC001u32.to_le_bytes().to_vec();
    body.extend_from_slice(&[0x00, 0xD8, 0x00, 0x00]);

    // oracle: spec MS-RDPECLIP §2.2.3.2 - a Format List that isn't processed is answered with CB_RESPONSE_FAIL.
    assert_eq!(
        process(&mut cliprdr, &format_list_pdu(UNICODE_NAMES, &body)),
        [FormatListResponse::Fail]
    );
    // oracle: mutant the backend left with the formats of the copy before
    assert_eq!(*copies.lock().unwrap(), [Vec::<ClipboardFormat>::new()]);
}
