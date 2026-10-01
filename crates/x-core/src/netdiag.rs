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
}
