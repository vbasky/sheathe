//! Protection System Specific Header (`pssh`) box generation.
//!
//! A `pssh` box carries the per-DRM-system data a player hands to its CDM to
//! obtain the content key. sheathe generates the boxes directly from the raw
//! key (the same path Shaka Packager's `--protection_systems` takes), so no key
//! server is involved:
//!
//! - **Common** (`1077efec…`) — a version-1 box listing the `KID`(s); no system
//!   data. The W3C clear-key / `urn:mpeg:dash:mp4protection` family.
//! - **Widevine** (`edef8ba9…`) — a version-0 box whose data is a
//!   `WidevinePsshData` protobuf: the `KID` and the protection-scheme fourcc.
//! - **PlayReady** (`9a04f079…`) — a version-0 box wrapping a PlayReady Object
//!   that carries a UTF-16LE `WRMHEADER` 4.0.0.0 (KID, key length, ALGID, and a
//!   checksum proving possession of the key).

use crate::{ContentKey, Scheme};
use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};

/// Common (clear-key family) System ID — `urn:mpeg:dash:mp4protection`.
const COMMON_SYSTEM_ID: [u8; 16] = [
    0x10, 0x77, 0xef, 0xec, 0xc0, 0xb2, 0x4d, 0x02, 0xac, 0xe3, 0x3c, 0x1e, 0x52, 0xe2, 0xfb, 0x4b,
];
/// Widevine System ID.
const WIDEVINE_SYSTEM_ID: [u8; 16] = [
    0xed, 0xef, 0x8b, 0xa9, 0x79, 0xd6, 0x4a, 0xce, 0xa3, 0xc8, 0x27, 0xdc, 0xd5, 0x1d, 0x21, 0xed,
];
/// PlayReady System ID.
const PLAYREADY_SYSTEM_ID: [u8; 16] = [
    0x9a, 0x04, 0xf0, 0x79, 0x98, 0x40, 0x42, 0x86, 0xab, 0x92, 0xe6, 0x5b, 0xe0, 0x88, 0x5f, 0x95,
];

/// A DRM protection system a `pssh` box can be generated for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionSystem {
    /// W3C Common / clear-key family (`urn:mpeg:dash:mp4protection`).
    Common,
    /// Google Widevine.
    Widevine,
    /// Microsoft PlayReady.
    PlayReady,
}

impl ProtectionSystem {
    /// Parse a case-insensitive system name (as used on the command line).
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "common" | "commonsystem" => Some(Self::Common),
            "widevine" => Some(Self::Widevine),
            "playready" => Some(Self::PlayReady),
            _ => None,
        }
    }

    /// The 16-byte DRM System ID written into the `pssh` box.
    pub fn system_id(self) -> [u8; 16] {
        match self {
            Self::Common => COMMON_SYSTEM_ID,
            Self::Widevine => WIDEVINE_SYSTEM_ID,
            Self::PlayReady => PLAYREADY_SYSTEM_ID,
        }
    }

    /// Build the complete `pssh` box for this system, protecting `key` under
    /// `scheme`.
    pub fn pssh_box(self, key: &ContentKey, scheme: Scheme) -> Vec<u8> {
        match self {
            // Version-1 box: KID list in the box header, no system data.
            Self::Common => assemble(self.system_id(), 1, &[key.kid], &[]),
            Self::Widevine => assemble(self.system_id(), 0, &[], &widevine_data(key, scheme)),
            Self::PlayReady => assemble(self.system_id(), 0, &[], &playready_data(key, scheme)),
        }
    }
}

/// Assemble a `pssh` box. Version 1 carries a `KID` list; version 0 omits it.
fn assemble(system_id: [u8; 16], version: u8, kids: &[[u8; 16]], data: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"pssh");
    body.push(version);
    body.extend_from_slice(&[0, 0, 0]); // flags
    body.extend_from_slice(&system_id);
    if version >= 1 {
        body.extend_from_slice(&(kids.len() as u32).to_be_bytes());
        for kid in kids {
            body.extend_from_slice(kid);
        }
    }
    body.extend_from_slice(&(data.len() as u32).to_be_bytes());
    body.extend_from_slice(data);

    let mut out = ((body.len() + 4) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(&body);
    out
}

/// `WidevinePsshData` protobuf: `key_id` (field 2) and `protection_scheme`
/// (field 9, the scheme fourcc as a big-endian `uint32`).
fn widevine_data(key: &ContentKey, scheme: Scheme) -> Vec<u8> {
    let mut data = Vec::new();
    data.push(0x12); // field 2 (key_id), wire type 2 (length-delimited)
    data.push(0x10); // length 16
    data.extend_from_slice(&key.kid);
    data.push(0x48); // field 9 (protection_scheme), wire type 0 (varint)
    put_varint(u32::from_be_bytes(scheme.scheme_type()), &mut data);
    data
}

/// Append `value` as a protobuf base-128 varint.
fn put_varint(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// A PlayReady Object wrapping a single Rights Management Header (`WRMHEADER`).
fn playready_data(key: &ContentKey, scheme: Scheme) -> Vec<u8> {
    let kid = playready_guid(&key.kid);
    let kid_b64 = base64(&kid);
    let checksum_b64 = base64(&playready_checksum(&key.key, &kid));
    // CTR schemes use AESCTR; CBC schemes use AESCBC.
    let algid = if scheme.is_cbc() { "AESCBC" } else { "AESCTR" };

    let xml = format!(
        "<WRMHEADER xmlns=\"http://schemas.microsoft.com/DRM/2007/03/PlayReadyHeader\" \
         version=\"4.0.0.0\"><DATA><PROTECTINFO><KEYLEN>16</KEYLEN><ALGID>{algid}</ALGID>\
         </PROTECTINFO><KID>{kid_b64}</KID><CHECKSUM>{checksum_b64}</CHECKSUM></DATA></WRMHEADER>"
    );
    let header: Vec<u8> = xml.encode_utf16().flat_map(u16::to_le_bytes).collect();

    // One record: type 1 (Rights Management Header) + length + UTF-16LE header.
    let record_len = header.len() as u16;
    // PlayReady Object: total length (incl. itself) + record count + record.
    let total_len = 4 + 2 + 2 + 2 + header.len();
    let mut obj = Vec::with_capacity(total_len);
    obj.extend_from_slice(&(total_len as u32).to_le_bytes());
    obj.extend_from_slice(&1u16.to_le_bytes()); // record count
    obj.extend_from_slice(&1u16.to_le_bytes()); // record type: RM header
    obj.extend_from_slice(&record_len.to_le_bytes());
    obj.extend_from_slice(&header);
    obj
}

/// Reorder a `KID` into PlayReady's little-endian GUID byte order (the first
/// three GUID fields are byte-swapped; the trailing eight bytes are unchanged).
fn playready_guid(kid: &[u8; 16]) -> [u8; 16] {
    let mut g = *kid;
    g.swap(0, 3);
    g.swap(1, 2);
    g.swap(4, 5);
    g.swap(6, 7);
    g
}

/// PlayReady header checksum: the first 8 bytes of AES-128-ECB encrypting the
/// (GUID-ordered) `KID` with the content key.
fn playready_checksum(key: &[u8; 16], guid_kid: &[u8; 16]) -> [u8; 8] {
    let cipher = Aes128::new(GenericArray::from_slice(key));
    let mut block = GenericArray::clone_from_slice(guid_kid);
    cipher.encrypt_block(&mut block);
    block[..8].try_into().unwrap()
}

/// Standard Base64 encoding (RFC 4648, with `=` padding).
fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18 & 0x3f) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6 & 0x3f) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[(n & 0x3f) as usize] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // KID/KEY used to capture the Shaka Packager oracle bytes below.
    const KID: [u8; 16] = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0x00, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ];
    const KEY: [u8; 16] = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ];

    fn key() -> ContentKey {
        ContentKey { kid: KID, key: KEY }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn common_pssh_matches_shaka() {
        let got = ProtectionSystem::Common.pssh_box(&key(), Scheme::Cenc);
        assert_eq!(
            hex(&got),
            "0000003470737368010000001077efecc0b24d02ace33c1e52e2fb4b\
             0000000111223344556677889900aabbccddeeff00000000"
        );
    }

    #[test]
    fn widevine_pssh_matches_shaka() {
        let got = ProtectionSystem::Widevine.pssh_box(&key(), Scheme::Cenc);
        assert_eq!(
            hex(&got),
            "000000387073736800000000edef8ba979d64acea3c827dcd51d21ed\
             00000018121011223344556677889900aabbccddeeff48e3dc959b06"
        );
    }

    #[test]
    fn playready_pssh_matches_shaka() {
        let got = ProtectionSystem::PlayReady.pssh_box(&key(), Scheme::Cenc);
        // Full box captured byte-for-byte from Shaka `--protection_systems
        // PlayReady` (the UTF-16LE WRMHEADER, swapped-GUID KID, and checksum).
        let expected = concat!(
            "0000022670737368000000009a04f07998404286ab92e65be0885f9500000206060200000100010",
            "0fc013c00570052004d00480045004100440045005200200078006d006c006e0073003d002200680",
            "07400740070003a002f002f0073006300680065006d00610073002e006d006900630072006f0073",
            "006f00660074002e0063006f006d002f00440052004d002f00320030003000370",
            "02f00300033002f0050006c0061007900520065006100640079004800650061006",
            "4006500720022002000760065007200730069006f006e003d00220034002e0030",
            "002e0030002e00300022003e003c0044004100540041003e003c00500052004f005",
            "40045004300540049004e0046004f003e003c004b00450059004c0045004e003e0",
            "0310036003c002f004b00450059004c0045004e003e003c0041004c00470049004",
            "4003e004100450053004300540052003c002f0041004c004700490044003e003c0",
            "02f00500052004f00540045004300540049004e0046004f003e003c004b0049004",
            "4003e00520044004d006900450057005a0056006900480065005a0041004b00710",
            "037007a004e00330075002f0077003d003d003c002f004b00490044003e003c004",
            "3004800450043004b00530055004d003e00350041006100550053004600700056",
            "004800640030003d003c002f0043004800450043004b00530055004d003e003c00",
            "2f0044004100540041003e003c002f00570052004d004800450041004400450052003e00",
        );
        assert_eq!(hex(&got), expected);
    }

    #[test]
    fn widevine_protection_scheme_tracks_fourcc() {
        // Field 9 (protection_scheme) is the scheme fourcc as a big-endian u32,
        // so cbcs differs from cenc only in the trailing varint while sharing the
        // KID prefix (field 2).
        let cenc = ProtectionSystem::Widevine.pssh_box(&key(), Scheme::Cenc);
        let cbcs = ProtectionSystem::Widevine.pssh_box(&key(), Scheme::Cbcs);
        assert_ne!(cenc, cbcs);
        let kid_prefix = hex(&[&[0x12, 0x10][..], &KID].concat());
        assert!(hex(&cenc).contains(&kid_prefix) && hex(&cbcs).contains(&kid_prefix));
    }

    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!(ProtectionSystem::parse("Widevine"), Some(ProtectionSystem::Widevine));
        assert_eq!(ProtectionSystem::parse("PLAYREADY"), Some(ProtectionSystem::PlayReady));
        assert_eq!(ProtectionSystem::parse("commonsystem"), Some(ProtectionSystem::Common));
        assert_eq!(ProtectionSystem::parse("nope"), None);
    }
}
