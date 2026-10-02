//! Certificate, TLS and HTTP diagnostics: models over the platform's own
//! network tools.
//!
//! `x` deliberately does not bundle a TLS stack. Handshakes and requests run
//! through what the OS already ships (`curl`, `openssl s_client`, the
//! Schannel-backed `SslStream`), and every field below is what that tool
//! actually reported — a `None` means "the tool did not say", never a guess.
//! Verification results are the platform store's own verdict.

use serde::{Deserialize, Serialize};

/// TLS facts for one `host:port`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TlsInfo {
    /// Host that was connected to.
    pub host: String,
    /// Port that answered.
    pub port: u16,
    /// Negotiated protocol, e.g. `TLSv1.3` / `Tls13`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// Negotiated cipher suite, when the tool prints one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cipher: Option<String>,
    /// Leaf certificate subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Leaf certificate issuer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// Validity start, UTC text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_before: Option<String>,
    /// Validity end, UTC text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_after: Option<String>,
    /// Subject Alternative Name entries (DNS / IP), when recoverable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub san: Vec<String>,
    /// Whether the platform trust store accepted the chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified: Option<bool>,
    /// The store's own words: `Verify return code: 0 (ok)` or chain status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify_detail: Option<String>,
}

/// One HTTP response, probed by the platform's HTTP client.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HttpResponse {
    /// URL that was requested.
    pub url: String,
    /// Status code, when the server answered one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// HTTP version of the response line (`HTTP/2 200`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_version: Option<String>,
    /// Response headers, in the order they arrived.
    pub headers: Vec<(String, String)>,
    /// Total transfer time in milliseconds, when the tool measured it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_total_ms: Option<f64>,
    /// Remote IP the answer came from, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_ip: Option<String>,
    /// Bytes downloaded (body), when the tool counted them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

/// One stage of the `x net check` chain (DNS → TCP → TLS → cert → HTTP).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagStep {
    /// Stage name: `dns`, `tcp`, `tls`, `cert`, `http`.
    pub stage: String,
    /// `ok` / `fail` / `warn`.
    pub verdict: String,
    /// What was observed, in one line.
    pub detail: String,
}

impl DiagStep {
    /// A step with a verdict and its evidence.
    pub fn new(stage: &str, verdict: &str, detail: impl Into<String>) -> Self {
        Self {
            stage: stage.to_string(),
            verdict: verdict.to_string(),
            detail: detail.into(),
        }
    }

    /// ✓ / ✗ / ! marker matching the verdict.
    pub fn mark(&self) -> &'static str {
        match self.verdict.as_str() {
            "ok" => "✓",
            "warn" => "!",
            _ => "✗",
        }
    }

    /// Whether the chain has no failed stage (`warn` still passes).
    pub fn all_ok(steps: &[DiagStep]) -> bool {
        !steps.iter().any(|step| step.verdict == "fail")
    }
}

/// One measured transfer, and what it works out to per second.
///
/// This is a throughput *sample*, not a link-speed reading: it says what this
/// connection actually moved in this run. The two are deliberately separate
/// fields so a fast sample on a slow link stays legible.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeedSample {
    /// URL that was fetched.
    pub url: String,
    /// Bytes actually downloaded.
    pub bytes: u64,
    /// Seconds the whole transfer took, as the tool measured them.
    pub seconds: f64,
    /// Download rate in bytes per second, rounded.
    pub bytes_per_second: u64,
    /// The same rate in bits per second, what a link rate is quoted in.
    pub bits_per_second: u64,
}

impl SpeedSample {
    /// A sample from raw measurements, refusing a rate that would be a lie.
    ///
    /// A zero or negative duration means the tool did not measure time; a
    /// division by it would be `inf` or a NaN, so it is refused instead.
    pub fn new(url: impl Into<String>, bytes: u64, seconds: f64) -> Option<Self> {
        if !seconds.is_finite() || seconds <= 0.0 {
            return None;
        }
        let bytes_per_second = (bytes as f64 / seconds).round();
        if !bytes_per_second.is_finite() || bytes_per_second < 0.0 {
            return None;
        }
        let bytes_per_second = bytes_per_second as u64;
        Some(Self {
            url: url.into(),
            bytes,
            seconds,
            bytes_per_second,
            bits_per_second: bytes_per_second.saturating_mul(8),
        })
    }

    /// Human speed, in the unit a link rate is normally quoted in.
    pub fn human_bits_per_second(&self) -> String {
        format_bits(self.bits_per_second)
    }
}

/// Bits per second as a human string: `12.3 Mbit/s`.
pub fn format_bits(bits_per_second: u64) -> String {
    const UNITS: [&str; 5] = ["bit/s", "Kbit/s", "Mbit/s", "Gbit/s", "Tbit/s"];
    let mut value = bits_per_second as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bits_per_second} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_info_skips_silent_fields() {
        let info = TlsInfo {
            host: "example.com".into(),
            port: 443,
            protocol: Some("TLSv1.3".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&info).expect("json");
        assert!(json.contains("\"protocol\":\"TLSv1.3\""));
        assert!(
            !json.contains("cipher"),
            "a field the tool did not answer stays out: {json}"
        );
        assert!(!json.contains("san"));
    }

    #[test]
    fn verdict_marks_and_chain_health() {
        let steps = vec![
            DiagStep::new("dns", "ok", "2 addresses"),
            DiagStep::new("tcp", "warn", "slow"),
        ];
        assert_eq!(steps[0].mark(), "✓");
        assert_eq!(steps[1].mark(), "!");
        assert!(DiagStep::all_ok(&steps));
        let broken = vec![DiagStep::new("tls", "fail", "handshake reset")];
        assert!(!DiagStep::all_ok(&broken));
    }

    // --- throughput -------------------------------------------------------

    #[test]
    fn a_sample_reports_bytes_and_bits_per_second() {
        // 1 MB in 1 s is 1 MiB/s, which is 8 MiB/s in bits.
        let sample = SpeedSample::new("https://example.test/x", 1_048_576, 1.0).expect("sample");
        assert_eq!(sample.bytes_per_second, 1_048_576);
        assert_eq!(sample.bits_per_second, 8_388_608);
        assert_eq!(sample.human_bits_per_second(), "8.4 Mbit/s");
    }

    #[test]
    fn a_half_second_transfer_doubles_the_rate() {
        let sample = SpeedSample::new("https://example.test/x", 1_000_000, 0.5).expect("sample");
        assert_eq!(sample.bytes_per_second, 2_000_000);
    }

    #[test]
    fn an_unmeasured_duration_is_refused_rather_than_dividing_by_zero() {
        // curl prints `0.000000` when it could not time the transfer; a
        // division by that would be `inf`, which is a number nobody can act on.
        assert!(SpeedSample::new("https://example.test/x", 10, 0.0).is_none());
        assert!(SpeedSample::new("https://example.test/x", 10, -1.0).is_none());
        assert!(SpeedSample::new("https://example.test/x", 10, f64::NAN).is_none());
        assert!(SpeedSample::new("https://example.test/x", 10, f64::INFINITY).is_none());
    }

    #[test]
    fn an_empty_body_still_yields_a_zero_rate_not_a_failure() {
        // A 204 answered instantly is a real measurement of nothing moved.
        let sample = SpeedSample::new("https://example.test/x", 0, 0.4).expect("sample");
        assert_eq!(sample.bytes_per_second, 0);
        assert_eq!(sample.bits_per_second, 0);
        assert_eq!(sample.human_bits_per_second(), "0 bit/s");
    }

    #[test]
    fn bits_are_quoted_in_the_unit_a_link_rate_uses() {
        assert_eq!(format_bits(0), "0 bit/s");
        assert_eq!(format_bits(999), "999 bit/s");
        assert_eq!(format_bits(1_000), "1.0 Kbit/s");
        assert_eq!(format_bits(1_500_000), "1.5 Mbit/s");
        assert_eq!(format_bits(2_500_000_000), "2.5 Gbit/s");
        assert_eq!(format_bits(1_000_000_000_000), "1.0 Tbit/s");
    }

    #[test]
    fn a_saturated_rate_does_not_overflow_into_a_wrong_number() {
        // `u64::MAX / 4` bytes a second overflows when multiplied by eight; the
        // rate must saturate rather than wrap to a tiny number.
        let sample = SpeedSample::new("https://example.test/x", u64::MAX / 4, 1.0).expect("sample");
        assert_eq!(sample.bits_per_second, u64::MAX);
    }
}
