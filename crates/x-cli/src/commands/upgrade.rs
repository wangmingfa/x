//! `x upgrade`: replace this binary with the newest GitHub release.
//!
//! The release workflow (`.github/workflows/release.yml`) publishes one
//! tar.gz per platform; this command mirrors what `scripts/install-release.sh`
//! does, but from inside the binary being replaced:
//!
//! * resolve the newest release tag from `/releases` (not `/releases/latest`:
//!   that endpoint 404s while the newest release is a prerelease),
//! * pick the asset for the running platform (macOS ships a universal binary,
//!   Linux ships one per architecture, so no runtime arch check is needed),
//! * download, unpack, and swap the running executable in place.
//!
//! Windows replaces `x.exe` via a rename dance: a running executable cannot
//! be overwritten, but it can be renamed, and the new file takes the old name.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use clap::Parser;
use x_core::error::{Error, ErrorKind, Result};

use crate::format::{Confirmer, Renderer};
use crate::{commands, OutputFormat};

/// GitHub repository to pull releases from.
const REPO: &str = "wangmingfa/x";

#[derive(Debug, Parser)]
pub struct UpgradeArgs {
    /// Upgrade even if the newest release matches the running version.
    #[arg(long)]
    pub force: bool,

    /// Assume yes: replace the binary without asking.
    #[arg(short, long)]
    pub yes: bool,
}

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(concat!("x/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn fail(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::System, message)
}

/// Asset name for the platform this binary was built for.
fn asset_for_current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS-universal"
    } else if cfg!(target_os = "linux") {
        match std::env::consts::ARCH {
            "aarch64" => "Linux-aarch64",
            _ => "Linux-x86_64",
        }
    } else {
        "windows-x86_64"
    }
}

/// Tag of the newest release, prereleases included.
fn newest_tag() -> Result<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases");
    let body = http_agent()
        .get(&url)
        .call()
        .map_err(|e| fail(format!("release lookup failed: {e}")))?
        .into_string()
        .map_err(|e| fail(format!("release lookup read failed: {e}")))?;
    let releases: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| fail(format!("release listing is not JSON: {e}")))?;
    releases
        .as_array()
        .and_then(|list| list.first())
        .and_then(|first| first.get("tag_name"))
        .and_then(|tag| tag.as_str())
        .map(str::to_owned)
        .ok_or_else(|| fail("no releases published yet"))
}

/// Download the release tarball into `dest` and return its path.
fn download(tag: &str, dest: &Path) -> Result<PathBuf> {
    let asset = asset_for_current_platform();
    let url = format!("https://github.com/{REPO}/releases/download/{tag}/x-{tag}-{asset}.tar.gz");
    let response = http_agent()
        .get(&url)
        .call()
        .map_err(|e| fail(format!("download failed: {e}")))?;
    let mut file = fs::File::create(dest)?;
    std::io::copy(&mut response.into_reader(), &mut file)?;
    file.flush()?;
    if fs::metadata(dest)?.len() == 0 {
        return Err(fail("downloaded archive is empty"));
    }
    Ok(dest.to_path_buf())
}

/// Check the downloaded archive against the release's `SHA256SUMS`.
///
/// Returns `Ok(false)` when the release does not publish a checksum file
/// (older releases predate it) — reported, not fatal. A *mismatch* is fatal:
/// the whole point of replacing the running binary is not to do it on a
/// corrupted or tampered download.
fn verify_checksum(tag: &str, archive: &Path) -> Result<bool> {
    let asset = asset_for_current_platform();
    let name = format!("x-{tag}-{asset}.tar.gz");
    let url = format!("https://github.com/{REPO}/releases/download/{tag}/SHA256SUMS");
    let Ok(response) = http_agent().get(&url).call() else {
        // The checksum file is optional for old releases; the caller warns.
        return Ok(false);
    };
    let sums = response
        .into_string()
        .map_err(|e| fail(format!("cannot read SHA256SUMS: {e}")))?;
    let wanted = sums
        .lines()
        .find_map(|line| {
            // sha256sum style: `<hex>  <filename>` — two spaces.
            let (hex, file) = line.split_once(' ')?;
            file.trim().eq_ignore_ascii_case(&name).then(|| hex.trim())
        })
        .ok_or_else(|| fail(format!("SHA256SUMS does not list {name}")))?;
    let digest = x_core::file::hash_file(archive, x_core::file::HashAlgo::Sha256)
        .map_err(|e| fail(format!("cannot hash the download: {e}")))?;
    if !digest.hex.eq_ignore_ascii_case(wanted) {
        return Err(fail(format!(
            "checksum mismatch for {name}: expected {wanted}, got {}",
            digest.hex
        )));
    }
    Ok(true)
}

/// Extract the `x` binary from the tarball into `dest_dir` and return its path.
fn unpack(archive: &Path, dest_dir: &Path) -> Result<PathBuf> {
    let gz = fs::File::open(archive)?;
    let mut tarball = tar::Archive::new(flate2::read::GzDecoder::new(gz));
    for entry in tarball.entries()? {
        let mut entry = entry?;
        let name = entry
            .path()?
            .file_name()
            .map(|n| n.to_owned())
            .unwrap_or_default();
        // The workflow packs a bare `x`; accept it under any leading dir, but
        // nothing else.
        if name != std::ffi::OsStr::new("x") {
            continue;
        }
        let out = dest_dir.join("x");
        let mut out_file = fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut out_file)?;
        out_file.flush()?;
        return Ok(out);
    }
    Err(fail("archive did not contain the x binary"))
}

/// Replace the running executable with `new_bin`.
fn replace_current_exe(new_bin: &Path) -> Result<PathBuf> {
    let current =
        std::env::current_exe().map_err(|e| fail(format!("cannot locate the running x: {e}")))?;
    let dir = current
        .parent()
        .ok_or_else(|| fail("running executable has no parent directory"))?;

    #[cfg(windows)]
    {
        // A running x.exe cannot be overwritten, but a renamed one keeps
        // running happily: move the old file aside, then bring the new one in.
        let name = current
            .file_name()
            .ok_or_else(|| fail("running executable has no file name"))?
            .to_os_string();
        let old = dir.join(format!("{}.old", name.to_string_lossy()));
        let _ = fs::remove_file(&old);
        fs::rename(&current, &old)?;
        fs::copy(new_bin, dir.join(&name))?;
    }
    #[cfg(not(windows))]
    {
        // The running binary may be named anything (`x`, `x-cli`, a distro
        // package name), so replace the exact file current_exe() names — and
        // do it with a rename, not an in-place copy: writing into a live
        // text image fails with ETXTBSY, while a staged sibling renamed over
        // it succeeds atomically. fs::copy carries the file mode over.
        let staged = dir.join(format!(".x-new-{}", std::process::id()));
        let _ = fs::remove_file(&staged);
        fs::copy(new_bin, &staged)?;
        fs::rename(&staged, &current)?;
    }
    Ok(dir.to_path_buf())
}

/// Print the upgrade report in the caller's output format.
fn report(renderer: &mut Renderer, doc: serde_json::Value, text: String) -> Result<i32> {
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&doc)?;
    } else {
        renderer.line(text)?;
    }
    Ok(0)
}

pub fn dispatch(
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    args: &UpgradeArgs,
) -> Result<i32> {
    // Dry-run first, before any network: the plan is a question about the
    // future, not a reason to touch GitHub.
    let running = env!("CARGO_PKG_VERSION").trim_start_matches('v');
    if crate::dry_run_guard(
        renderer,
        &format!("check for the latest x release and upgrade from {running}"),
    )? {
        return Ok(0);
    }
    let tag = newest_tag()?;
    let running = env!("CARGO_PKG_VERSION").trim_start_matches('v');
    let latest = tag.trim_start_matches('v');
    if latest == running && !args.force {
        return report(
            renderer,
            serde_json::json!({ "up_to_date": true, "version": running }),
            format!("already up to date ({running})"),
        );
    }

    // Replacing the running binary is the one destructive thing here.
    let action = format!("upgrade x {running} -> {latest}");
    if !commands::confirm(renderer, confirmer, args.yes, &action)? {
        return Ok(commands::EXIT_DECLINED);
    }

    // An unpredictable temp path: a fixed name under /tmp could be
    // pre-planted (symlink) by another local user, and File::create would
    // follow it. Timestamp + pid are enough: the directory did not exist a
    // moment ago and is gone again below.
    let unique = format!(
        "{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
        std::process::id()
    );
    let tmp = std::env::temp_dir().join(format!("x-upgrade-{latest}-{unique}"));
    fs::create_dir_all(&tmp)?;
    let archive = download(&tag, &tmp.join("x.tar.gz"))?;
    if verify_checksum(&tag, &archive)? {
        renderer.line("checksum ok (SHA256SUMS)")?;
    } else {
        renderer.line("note: this release publishes no SHA256SUMS; cannot verify the download")?;
    }
    let new_bin = unpack(&archive, &tmp)?;
    let dir = replace_current_exe(&new_bin)?;
    let _ = fs::remove_dir_all(&tmp);

    report(
        renderer,
        serde_json::json!({ "upgraded": true, "from": running, "to": latest, "path": dir }),
        format!("upgraded {running} -> {latest} ({})", dir.display()),
    )
}
