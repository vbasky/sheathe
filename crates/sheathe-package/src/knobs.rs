//! Extra packaging knobs that map 1:1 onto Shaka Packager flags.

use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::descriptor::StreamDescriptor;

/// HLS / DASH / mux / DRM / IO knobs layered on [`crate::PackageOptions`].
#[derive(Debug, Clone)]
pub struct PackageKnobs {
    /// `--hls_base_url`.
    pub hls_base_url: Option<String>,
    /// `--hls_media_sequence_number` initial value (live window still offsets).
    pub hls_media_sequence_number: u64,
    /// `--hls_start_time_offset`.
    pub hls_start_time_offset: Option<f64>,
    /// `--create_session_keys`.
    pub create_session_keys: bool,
    /// `--add_program_date_time` (also implied for live/event).
    pub add_program_date_time: bool,
    /// `--closed_captions`.
    pub closed_captions: Vec<ClosedCaption>,
    /// `--use_legacy_vp9_codec_string`.
    pub use_legacy_vp9_codec_string: bool,
    /// `--dash_add_last_segment_number_when_needed`.
    pub dash_add_last_segment_number: bool,
    /// `--segment_template_constant_duration`.
    pub segment_template_constant_duration: bool,
    /// `--use_dovi_supplemental_codecs`.
    pub use_dovi_supplemental_codecs: bool,
    /// `--mvex_before_trak`.
    pub mvex_before_trak: bool,
    /// `--strip_parameter_set_nalus` (avc1/hvc1 vs avc3/hev1). Stored for muxers.
    pub strip_parameter_set_nalus: bool,
    /// `--crypt_byte_block` (pattern schemes). Default 1.
    pub crypt_byte_block: u8,
    /// `--skip_byte_block` (pattern schemes). Default 9.
    pub skip_byte_block: u8,
    /// `--playready_extra_header_data`.
    pub playready_extra_header_data: Option<String>,
    /// `--protection_scheme aes128` — HLS AES-128 full-segment CBC.
    pub hls_aes128: bool,
    /// `--keys label=AUDIO:key_id=…:key=…`.
    pub labeled_keys: Vec<LabeledKey>,
    /// `--decrypt` / `--enable_raw_key` decryption of encrypted inputs.
    pub decrypt_input: bool,
    /// Widevine license-server fetch.
    pub widevine: Option<WidevineConfig>,
    /// CPIX / SPEKE.
    pub cpix: Option<CpixConfig>,
    /// `--ignore_http_output_failures`.
    pub ignore_http_output_failures: bool,
    /// `--user_agent`.
    pub user_agent: Option<String>,
    /// `--ca_file`.
    pub ca_file: Option<PathBuf>,
    /// `--client_cert_file`.
    pub client_cert_file: Option<PathBuf>,
    /// `--client_cert_private_key_file`.
    pub client_cert_key_file: Option<PathBuf>,
    /// `--client_cert_private_key_password`.
    pub client_cert_key_password: Option<String>,
    /// `--disable_peer_verification`.
    pub disable_peer_verification: bool,
    /// Keep rewriting live manifests until interrupted.
    pub live_rewrite: bool,
    /// Stream descriptors (when non-empty, they replace bare input paths).
    pub descriptors: Vec<StreamDescriptor>,
    /// Pixel thresholds for auto DRM labels (Shaka defaults).
    pub max_sd_pixels: u32,
    pub max_hd_pixels: u32,
    pub max_uhd1_pixels: u32,
}

impl Default for PackageKnobs {
    fn default() -> Self {
        Self {
            hls_base_url: None,
            hls_media_sequence_number: 0,
            hls_start_time_offset: None,
            create_session_keys: false,
            add_program_date_time: false,
            closed_captions: Vec::new(),
            use_legacy_vp9_codec_string: false,
            dash_add_last_segment_number: false,
            segment_template_constant_duration: false,
            use_dovi_supplemental_codecs: false,
            mvex_before_trak: false,
            strip_parameter_set_nalus: true,
            crypt_byte_block: 1,
            skip_byte_block: 9,
            playready_extra_header_data: None,
            hls_aes128: false,
            labeled_keys: Vec::new(),
            decrypt_input: false,
            widevine: None,
            cpix: None,
            ignore_http_output_failures: false,
            user_agent: None,
            ca_file: None,
            client_cert_file: None,
            client_cert_key_file: None,
            client_cert_key_password: None,
            disable_peer_verification: false,
            live_rewrite: false,
            descriptors: Vec::new(),
            max_sd_pixels: 442_368,
            max_hd_pixels: 2_073_600,
            max_uhd1_pixels: 8_847_360,
        }
    }
}

/// One `--closed_captions` channel.
#[derive(Debug, Clone)]
pub struct ClosedCaption {
    pub instream_id: String,
    pub name: String,
    pub language: Option<String>,
    pub default: bool,
    pub autoselect: bool,
}

impl ClosedCaption {
    /// Parse Shaka `--closed_captions` (`channel=CC1,name=English,lang=eng;…`).
    pub fn parse_list(spec: &str) -> Result<Vec<Self>> {
        let mut out = Vec::new();
        for channel in spec.split(';').filter(|s| !s.is_empty()) {
            let mut cc = ClosedCaption {
                instream_id: "CC1".into(),
                name: "CC".into(),
                language: None,
                default: false,
                autoselect: true,
            };
            for kv in channel.split(',') {
                let Some((k, v)) = kv.split_once('=') else { continue };
                match k.trim() {
                    "channel" => cc.instream_id = v.trim().to_string(),
                    "name" => cc.name = v.trim().to_string(),
                    "lang" | "language" => cc.language = Some(v.trim().to_string()),
                    "default" => cc.default = v.trim() == "yes" || v.trim() == "1",
                    "autoselect" => cc.autoselect = v.trim() != "no" && v.trim() != "0",
                    _ => {}
                }
            }
            out.push(cc);
        }
        Ok(out)
    }
}

/// A labeled raw key (`--keys`).
#[derive(Debug, Clone)]
pub struct LabeledKey {
    pub label: String,
    pub kid: [u8; 16],
    pub key: [u8; 16],
    pub iv: Option<[u8; 16]>,
}

impl LabeledKey {
    /// Parse `--keys label=AUDIO:key_id=hex:key=hex,label=SD:…`.
    pub fn parse_list(spec: &str) -> Result<Vec<Self>> {
        let mut out = Vec::new();
        for entry in spec.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let mut label = String::new();
            let mut kid = None;
            let mut key = None;
            let mut iv = None;
            for kv in entry.split(':') {
                let Some((k, v)) = kv.split_once('=') else { continue };
                match k.trim() {
                    "label" => label = v.trim().to_string(),
                    "key_id" | "kid" => kid = Some(parse_hex16(v.trim())?),
                    "key" => key = Some(parse_hex16(v.trim())?),
                    "iv" => iv = Some(parse_hex16(v.trim())?),
                    _ => {}
                }
            }
            anyhow::ensure!(
                !label.is_empty() && kid.is_some() && key.is_some(),
                "keys entry needs label, key_id, key"
            );
            out.push(LabeledKey { label, kid: kid.unwrap(), key: key.unwrap(), iv });
        }
        Ok(out)
    }
}

/// Widevine Common Encryption key-server request.
#[derive(Debug, Clone)]
pub struct WidevineConfig {
    pub key_server_url: String,
    pub content_id: Vec<u8>,
    pub signer: String,
    pub aes_signing_key: Option<Vec<u8>>,
    pub aes_signing_iv: Option<Vec<u8>>,
    pub rsa_signing_key_pem: Option<String>,
    pub policy: String,
    pub group_id: Option<Vec<u8>>,
    pub enable_entitlement_license: bool,
    pub decrypt: bool,
}

/// CPIX document / SPEKE exchange.
#[derive(Debug, Clone)]
pub struct CpixConfig {
    pub path_or_url: String,
    pub headers: Vec<(String, String)>,
    pub private_key_pem: Option<String>,
    pub request_file: Option<PathBuf>,
    pub encrypt: bool,
    pub decrypt: bool,
}

pub(crate) fn parse_hex16(s: &str) -> Result<[u8; 16]> {
    let s = s.trim().trim_start_matches("0x");
    anyhow::ensure!(s.len() == 32, "expected 32 hex chars, got {}", s.len());
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).context("non-hex digit")?;
    }
    Ok(out)
}

/// Auto DRM label from resolution, matching Shaka defaults.
pub fn drm_label_for(
    kind: sheathe_core::MediaKind,
    pixels: u32,
    knobs: &PackageKnobs,
) -> &'static str {
    use sheathe_core::MediaKind;
    match kind {
        MediaKind::Audio | MediaKind::Text => "AUDIO",
        MediaKind::Video => {
            if pixels <= knobs.max_sd_pixels {
                "SD"
            } else if pixels <= knobs.max_hd_pixels {
                "HD"
            } else if pixels <= knobs.max_uhd1_pixels {
                "UHD1"
            } else {
                "UHD2"
            }
        }
    }
}
