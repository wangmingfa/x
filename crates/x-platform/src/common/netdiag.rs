//! Shared network-diagnosis probes: TLS and HTTP through the platform's own
//! tools.
//!
//! Layer 3 of the doctrine (`OS commands`) is the *only* honest option here:
//! `x` ships no TLS stack, so handshakes go to `curl`, to `openssl s_client`
//! on the unix family, and to the Schannel-backed `SslStream` on Windows.
//! Whatever those tools do not print stays `None` in the model — nothing here
//! paraphrases.

use x_core::error::{Error, ErrorKind, Result};
use x_core::netdiag::{HttpResponse, TlsInfo};

/// Probe `host:port` for TLS facts using the platform TLS client.
pub fn tls_info(host: &str, port: u16, timeout_ms: u64) -> Result<TlsInfo> {
    #[cfg(windows)]
    {
        let _ = timeout_ms;
        windows_tls(host, port)
    }
    #[cfg(not(windows))]
    {
        openssl_tls(host, port, timeout_ms)
    }
}

/// One HTTP request through `curl`, present on all three platforms.
pub fn http_probe(url: &str, method: &str, timeout_ms: u64) -> Result<HttpResponse> {
    let body_sink = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let seconds = (timeout_ms / 1000).max(1).to_string();
    let args = [
        "-sS",
        "-o",
        body_sink,
        "-D",
        "-",
        "--max-time",
        &seconds,
        "-X",
        method,
        "-w",
        "%{http_code}|%{time_total}|%{remote_ip}|%{size_download}",
        url,
    ];
    let (code, stdout, stderr) = run_curl(&args)?;
    let response = parse_curl_output(url, &stdout);
    if code != 0 {
        // The tool itself refused; report its verdict, not a fake response.
        let first_line = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("curl failed")
            .to_string();
        return Err(curl_error(code, &first_line));
    }
    // A request that never produced a status line is a failure, not an empty page.
    if response.status.is_none() {
        let detail = if stderr.trim().is_empty() {
            "curl returned no status line".to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(Error::system(detail));
    }
    Ok(response)
}

/// Map curl's exit codes onto `x`'s error kinds.
fn curl_error(code: i32, text: &str) -> Error {
    match code {
        6 => Error::not_found(format!("cannot resolve host: {text}")),
        7 => Error::not_found(format!("cannot connect: {text}")),
        28 => Error::new(ErrorKind::Timeout, text.to_string()),
        35 | 60 => Error::new(
            ErrorKind::InvalidState,
            format!("TLS/certificate problem: {text}"),
        ),
        _ => Error::system(format!("curl failed (exit {code}): {text}")),
    }
}

#[cfg(windows)]
fn run_curl(args: &[&str]) -> Result<(i32, String, String)> {
    let (code, stdout, stderr) = crate::sys::run_command_capture("curl", args)
        .map_err(|e| Error::system(format!("cannot run curl: {e}")))?;
    Ok((
        code,
        crate::windows::service::decode_console(&stdout),
        crate::windows::service::decode_console(&stderr),
    ))
}

#[cfg(not(windows))]
fn run_curl(args: &[&str]) -> Result<(i32, String, String)> {
    let output = std::process::Command::new("curl")
        .args(args)
        .output()
        .map_err(|e| Error::system(format!("cannot run curl: {e}")))?;
    Ok((
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// Parse curl's stdout: the `-D -` header dump followed by the `-w` line.
fn parse_curl_output(url: &str, stdout: &str) -> HttpResponse {
    let mut response = HttpResponse {
        url: url.to_string(),
        ..Default::default()
    };
    let lines: Vec<&str> = stdout.lines().map(str::trim_end).collect();
    // The -w line is the last non-empty line that looks like `NNN|...`.
    let stats_idx = lines.iter().rposition(|line| {
        !line.is_empty() && line.contains('|') && line.starts_with(|c: char| c.is_ascii_digit())
    });
    let header_end = stats_idx.unwrap_or(lines.len());
    let stats_line = stats_idx.map(|index| lines[index].to_string());
    for line in &lines[..header_end] {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("HTTP/") {
            // Last status line wins: informational replies come first.
            let mut tokens = rest.split_whitespace();
            if let Some(version) = tokens.next() {
                response.http_version = Some(format!("HTTP/{version}"));
            }
            response.status = tokens.next().and_then(|code| code.parse().ok());
            continue;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            if !key.is_empty() {
                response
                    .headers
                    .push((key.trim().to_string(), value.trim().to_string()));
            }
        }
    }
    if let Some(stats) = stats_line {
        let parts: Vec<&str> = stats.split('|').collect();
        if parts.len() == 4 {
            if response.status.is_none() {
                response.status = parts[0].parse().ok();
            }
            response.time_total_ms = parts[1].parse().ok().map(|s: f64| s * 1000.0);
            if !parts[2].is_empty() {
                response.remote_ip = Some(parts[2].to_string());
            }
            response.bytes = parts[3].parse().ok();
        }
    }
    response
}

// ---------------------------------------------------------------------------
// Unix: openssl s_client + x509
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
fn openssl_tls(host: &str, port: u16, _timeout_ms: u64) -> Result<TlsInfo> {
    use std::process::{Command, Stdio};
    let output = Command::new("openssl")
        .args([
            "s_client",
            "-connect",
            &format!("{host}:{port}"),
            "-servername",
            host,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|_| {
            Error::unsupported("openssl is not available here; install it to probe TLS")
        })?;
    // s_client exits non-zero on verification problems it still reports;
    // keep the parsed facts and only hard-fail when there is no output at all.
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let mut info = parse_s_client(host, port, &stdout);
    if info.protocol.is_none() && info.cipher.is_none() {
        let detail = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with("CONNECT("))
            .unwrap_or("handshake produced no session information");
        return Err(Error::system(format!("openssl s_client: {detail}")));
    }
    if let Some(pem) = extract_first_pem(&stdout) {
        if let Ok(details) = openssl_x509(&pem) {
            merge_x509(&mut info, &details);
        }
    }
    Ok(info)
}

/// Parse protocol / cipher / verify code out of `s_client` stdout.
#[cfg(not(windows))]
fn parse_s_client(host: &str, port: u16, stdout: &str) -> TlsInfo {
    let mut info = TlsInfo {
        host: host.to_string(),
        port,
        ..Default::default()
    };
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Protocol: ") {
            info.protocol = Some(rest.to_string());
        } else if let Some(pos) = line.find("Cipher is ") {
            info.cipher = Some(
                line[pos + "Cipher is ".len()..]
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
            );
        } else if let Some(rest) = line.strip_prefix("Verify return code: ") {
            let mut parts = rest.splitn(2, ' ');
            let code = parts.next().unwrap_or("");
            info.verified = code.parse::<i32>().ok().map(|code| code == 0);
            info.verify_detail = Some(format!("Verify return code: {rest}"));
        }
    }
    info
}

/// The first PEM certificate block in `s_client` output (the peer's leaf).
#[cfg(not(windows))]
fn extract_first_pem(text: &str) -> Option<String> {
    let begin = text.find("-----BEGIN CERTIFICATE-----")?;
    let end = text[begin..].find("-----END CERTIFICATE-----")?;
    Some(text[begin..begin + end + "-----END CERTIFICATE-----".len()].to_string())
}

/// `openssl x509 -noout -subject -issuer -dates [-ext subjectAltName]`.
#[cfg(not(windows))]
fn openssl_x509(pem: &str) -> Result<String> {
    let with_ext = run_x509(pem, &["-ext", "subjectAltName"])?;
    if with_ext.0 == 0 {
        return Ok(with_ext.1);
    }
    // LibreSSL and ancient openssl reject `-ext`; ask without it.
    let plain = run_x509(pem, &[])?;
    if plain.0 == 0 {
        return Ok(plain.1);
    }
    Err(Error::system("openssl x509 could not read the certificate"))
}

#[cfg(not(windows))]
fn run_x509(pem: &str, extra: &[&str]) -> Result<(i32, String)> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut args = vec!["x509", "-noout", "-subject", "-issuer", "-dates"];
    args.extend(extra.iter().copied());
    let mut child = Command::new("openssl")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::system(format!("cannot run openssl x509: {e}")))?;
    child
        .stdin
        .take()
        .expect("stdin pipe")
        .write_all(pem.as_bytes())
        .map_err(|e| Error::system(format!("cannot feed certificate: {e}")))?;
    let output = child
        .wait_with_output()
        .map_err(|e| Error::system(format!("openssl x509 failed: {e}")))?;
    Ok((
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    ))
}

/// Merge the `x509` field dump into `info`.
#[cfg(not(windows))]
fn merge_x509(info: &mut TlsInfo, text: &str) {
    let mut san_next = false;
    for line in text.lines() {
        let trimmed = line.trim();
        san_next = match trimmed.split_once('=') {
            Some(("subject", value)) => {
                info.subject = Some(value.to_string());
                false
            }
            Some(("issuer", value)) => {
                info.issuer = Some(value.to_string());
                false
            }
            Some(("notBefore", value)) => {
                info.not_before = Some(value.to_string());
                false
            }
            Some(("notAfter", value)) => {
                info.not_after = Some(value.to_string());
                false
            }
            _ => {
                if trimmed.contains("Subject Alternative Name:") {
                    true
                } else if san_next && !trimmed.is_empty() {
                    info.san = trimmed
                        .split(',')
                        .map(|entry| entry.trim().to_string())
                        .filter(|entry| !entry.is_empty())
                        .collect();
                    false
                } else {
                    false
                }
            }
        };
    }
}

// ---------------------------------------------------------------------------
// Windows: Schannel through PowerShell's SslStream
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn windows_tls(host: &str, port: u16) -> Result<TlsInfo> {
    let checked = checked_host(host)?;
    let script = winevent_cert_script(&checked, port);
    let (code, stdout, stderr) = crate::sys::run_command_capture(
        "powershell.exe",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
    )
    .map_err(|e| Error::system(format!("cannot run powershell.exe: {e}")))?;
    let text = crate::windows::service::decode_console(&stdout)
        .trim()
        .to_string();
    if code != 0 || text.is_empty() {
        let detail = crate::windows::service::decode_console(&stderr)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("powershell.exe produced no output")
            .to_string();
        return Err(Error::system(format!("SslStream probe failed: {detail}")));
    }
    if let Some(message) = text.strip_prefix("ERR:") {
        return Err(Error::system(format!(
            "TLS handshake failed: {}",
            message.trim()
        )));
    }
    parse_ssl_stream_json(host, port, &text)
}

/// Hosts that reach a script must be boring: names, digits, dots, dashes.
#[cfg(windows)]
fn checked_host(raw: &str) -> Result<String> {
    let host = raw.trim();
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b':' || b == b'_')
    {
        return Err(Error::invalid_input(format!(
            "`{raw}` is not a usable host name or address"
        )));
    }
    Ok(host.to_string())
}

#[cfg(windows)]
fn winevent_cert_script(host: &str, port: u16) -> String {
    format!(
        r#"$ErrorActionPreference='Continue';try{{$t=New-Object System.Net.Sockets.TcpClient;$t.Connect('{host}',{port});$s=New-Object System.Net.Security.SslStream($t.GetStream(),$false,{{param($a,$b,$c,$d)$true}});$s.AuthenticateAsClient('{host}');$cert=New-Object System.Security.Cryptography.X509Certificates.X509Certificate2($s.RemoteCertificate);$ch=New-Object System.Security.Cryptography.X509Certificates.X509Chain;$ch.ChainPolicy.RevocationMode=0;$okv=$ch.Build($cert);$san=@();$ex=@($cert.Extensions|Where-Object{{$_.Oid.Value -eq '2.5.29.17'}});if($ex[0] -and $ex[0].RawData -and $ex[0].RawData[0] -eq 0x30){{$d=$ex[0].RawData;$p=2;if($d[1] -band 0x80){{$p=2+($d[1]-band 0x7f)}};while($p -lt ($d.Length-1)){{$tg=$d[$p];$l=[int]$d[$p+1];$p=$p+2;if($l -band 0x80){{$n=$l-band 0x80;$l=0;for($j=0;$j -lt $n;$j++){{$l=$l*256+[int]$d[$p+$j]}};$p=$p+$n}};$v=$p+$l;if($v -gt $d.Length){{$v=$d.Length}};if(($tg -band 0x1f) -eq 2 -and $l -gt 0){{$san+=[Text.Encoding]::UTF8.GetString($d[$p..($v-1)])}};$p=$p+$l}}}};[pscustomobject]@{{protocol=[string]$s.SslProtocol;cipher=[string]$s.CipherAlgorithm;subject=$cert.Subject;issuer=$cert.Issuer;not_before=$cert.NotBefore.ToUniversalTime().ToString('o');not_after=$cert.NotAfter.ToUniversalTime().ToString('o');san=$san;verified=$okv;verify_detail=(($ch.ChainStatus|ForEach-Object{{$_.Status}}) -join ',')}}|ConvertTo-Json -Compress;$s.Dispose();$t.Close()}}catch{{$msg=$_.Exception.Message;if($msg -notmatch '[\r\n]'){{Write-Output ('ERR:'+$msg)}}else{{Write-Output 'ERR:handshake failed'}}}}"#
    )
}

/// Turn the SslStream JSON into the model.
#[cfg(windows)]
fn parse_ssl_stream_json(host: &str, port: u16, text: &str) -> Result<TlsInfo> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| Error::system(format!("cannot parse the SslStream answer: {e}")))?;
    let text_of = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_string);
    Ok(TlsInfo {
        host: host.to_string(),
        port,
        protocol: text_of("protocol"),
        cipher: text_of("cipher"),
        subject: text_of("subject"),
        issuer: text_of("issuer"),
        not_before: text_of("not_before"),
        not_after: text_of("not_after"),
        san: value
            .get("san")
            .and_then(|v| match v {
                serde_json::Value::Array(rows) => Some(
                    rows.iter()
                        .filter_map(|row| row.as_str().map(str::to_string))
                        .collect(),
                ),
                serde_json::Value::String(one) => Some(vec![one.clone()]),
                _ => None,
            })
            .unwrap_or_default(),
        verified: value.get("verified").and_then(|v| v.as_bool()),
        // An empty ChainStatus means "nothing flagged"; do not invent words.
        verify_detail: text_of("verify_detail").filter(|detail| !detail.is_empty()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curl_headers_and_stats_are_split_back_out() {
        let stdout = concat!(
            "HTTP/2 301\r\n",
            "location: https://example.com/\r\n",
            "content-length: 0\r\n",
            "\r\n",
            "301|0.042138|203.0.113.5|0"
        );
        let response = parse_curl_output("http://example.com", stdout);
        assert_eq!(response.http_version.as_deref(), Some("HTTP/2"));
        assert_eq!(response.status, Some(301));
        assert_eq!(response.headers.len(), 2);
        assert_eq!(response.headers[0].0, "location");
        assert_eq!(response.time_total_ms.expect("ms").round(), 42.0);
        assert_eq!(response.remote_ip.as_deref(), Some("203.0.113.5"));
        assert_eq!(response.bytes, Some(0));
    }

    #[test]
    fn informational_responses_do_not_win() {
        let stdout = concat!(
            "HTTP/1.1 100 Continue\r\n",
            "\r\n",
            "HTTP/1.1 200 OK\r\n",
            "server: nginx\r\n",
            "\r\n",
            "200|1.5|10.0.0.1|12"
        );
        let response = parse_curl_output("http://api", stdout);
        assert_eq!(response.status, Some(200));
        assert_eq!(response.http_version.as_deref(), Some("HTTP/1.1"));
        assert!(response
            .headers
            .contains(&("server".to_string(), "nginx".to_string())));
    }

    #[test]
    fn curl_exit_codes_become_distinct_errors() {
        assert_eq!(
            curl_error(6, "could not resolve").kind(),
            ErrorKind::NotFound
        );
        assert_eq!(curl_error(28, "timed out").kind(), ErrorKind::Timeout);
        assert_eq!(curl_error(60, "peer cert").kind(), ErrorKind::InvalidState);
        assert_eq!(curl_error(56, "reset").kind(), ErrorKind::System);
    }
}
