//! Unified file operations.
//!
//! Inspection (`info`, `size`, `copy`, `move`, `rename`) is plain `std::fs`
//! and therefore implemented once here in `x-core`. The three verbs whose
//! behaviour genuinely differs per OS — `open` (double click), `reveal`
//! (select in the file manager) and `trash` (move to the recycle bin) — are
//! the required methods each platform adapter must provide.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What kind of entry a path refers to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileType {
    /// Regular file.
    #[default]
    File,
    /// Directory.
    Dir,
    /// Symbolic link (reported before the target is resolved).
    Symlink,
    /// Anything else (device, fifo, socket, ...).
    Other,
}

impl FileType {
    /// Lowercase identifier.
    pub const fn name(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Dir => "dir",
            Self::Symlink => "symlink",
            Self::Other => "other",
        }
    }
}

/// Everything `x file info` reports about one path.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileInfo {
    /// Path as requested (not canonicalized).
    pub path: PathBuf,
    /// Entry kind.
    pub file_type: FileType,
    /// Size in bytes; `None` when the platform cannot tell (e.g. some specials).
    pub size: Option<u64>,
    /// Unix mode in octal (e.g. `644`), `None` on Windows.
    pub mode: Option<String>,
    /// Whether the current process may write to the entry.
    pub readonly: bool,
    /// Owner user name, when the platform exposes one.
    pub owner: Option<String>,
    /// Owning group name, when the platform exposes one.
    pub group: Option<String>,
    /// Last modification time, RFC 3339.
    pub modified: Option<String>,
    /// Creation ("birth") time, RFC 3339, when the filesystem records it.
    pub created: Option<String>,
    /// Symbolic link target for `FileType::Symlink`.
    pub target: Option<PathBuf>,
}

impl FileInfo {
    /// Human readable type label used by tables.
    pub fn type_label(&self) -> &'static str {
        match self.file_type {
            FileType::File => "file",
            FileType::Dir => "directory",
            FileType::Symlink => "symlink",
            FileType::Other => "special",
        }
    }
}

/// One entry of a directory listing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirEntryInfo {
    /// File name (no parent path).
    pub name: String,
    /// Full path.
    pub path: PathBuf,
    /// Entry kind.
    pub file_type: FileType,
    /// Size in bytes (files only; directories report `None`).
    pub size: Option<u64>,
}

/// File inspection and the platform specific "open" family.
pub trait FileManager: Send + Sync {
    /// Metadata for one path.
    fn info(&self, path: &Path) -> Result<FileInfo> {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|e| Error::not_found(format!("{}: {e}", path.display())))?;
        Ok(file_info(path, &metadata))
    }

    /// Recursive byte count. Directories are walked; unreadable entries are
    /// skipped rather than failing the whole command.
    fn size(&self, path: &Path) -> Result<u64> {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|e| Error::not_found(format!("{}: {e}", path.display())))?;
        Ok(recursive_size(path, &metadata))
    }

    /// Names inside a directory, sorted, hidden entries included.
    fn list(&self, path: &Path) -> Result<Vec<DirEntryInfo>> {
        let mut rows = Vec::new();
        let entries = std::fs::read_dir(path)
            .map_err(|e| Error::not_found(format!("{}: {e}", path.display())))?;
        for entry in entries.flatten() {
            let file_type = match entry.file_type() {
                Ok(ft) if ft.is_symlink() => FileType::Symlink,
                Ok(ft) if ft.is_dir() => FileType::Dir,
                Ok(ft) if ft.is_file() => FileType::File,
                _ => FileType::Other,
            };
            let size = entry
                .metadata()
                .ok()
                .filter(|_| file_type == FileType::File)
                .map(|m| m.len());
            rows.push(DirEntryInfo {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: entry.path(),
                file_type,
                size,
            });
        }
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(rows)
    }

    /// Open the path the way a double click would (default application).
    fn open(&self, path: &Path) -> Result<()>;

    /// Show the path in the platform file manager, selecting it when the
    /// platform supports that (macOS Finder, Windows Explorer).
    fn reveal(&self, path: &Path) -> Result<()>;

    /// Move the path to the platform trash / recycle bin.
    fn trash(&self, path: &Path) -> Result<()>;

    /// Platform-native access control detail (Windows ACL, POSIX ACL), when
    /// the platform can produce one cheaply. `None` means "only the basic
    /// mode bits apply" or "no acl tool present".
    fn acl(&self, _path: &Path) -> Result<Option<String>> {
        Ok(None)
    }

    /// Copy a file or a directory tree. Existing destinations are overwritten.
    fn copy(&self, from: &Path, to: &Path) -> Result<()> {
        copy_path(from, to)
    }

    /// Move (or rename) a file or directory.
    fn move_path(&self, from: &Path, to: &Path) -> Result<()> {
        move_path(from, to)
    }

    /// Rename inside the same directory; convenience over [`Self::move_path`].
    fn rename(&self, path: &Path, new_name: &str) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| Error::invalid_input(format!("{} has no parent", path.display())))?;
        if new_name.contains(['/', '\\']) {
            return Err(Error::invalid_input(
                "new name must not contain a path separator",
            ));
        }
        move_path(path, &parent.join(new_name))
    }
}

/// Build a [`FileInfo`] from already fetched metadata.
///
/// Public so platform adapters that need richer owner information can extend
/// the result instead of re-deriving it.
pub fn file_info(path: &Path, metadata: &std::fs::Metadata) -> FileInfo {
    let file_type = if metadata.file_type().is_symlink() {
        FileType::Symlink
    } else if metadata.is_dir() {
        FileType::Dir
    } else if metadata.is_file() {
        FileType::File
    } else {
        FileType::Other
    };
    FileInfo {
        path: path.to_path_buf(),
        file_type,
        size: metadata.is_file().then_some(metadata.len()),
        mode: None,
        readonly: metadata.permissions().readonly(),
        owner: None,
        group: None,
        modified: system_time_to_rfc3339(metadata.modified().ok()),
        created: system_time_to_rfc3339(metadata.created().ok()),
        target: std::fs::read_link(path).ok(),
    }
}

/// System time → RFC 3339, `None` when the clock is before 1970.
pub fn system_time_to_rfc3339(time: Option<std::time::SystemTime>) -> Option<String> {
    let seconds = time?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(crate::audit::rfc3339_utc(seconds))
}

/// A checksum algorithm, named the way its own tools name it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HashAlgo {
    /// SHA-256: the integrity check every tool agrees on.
    #[default]
    Sha256,
    /// SHA-512, for callers that need the wider digest.
    Sha512,
    /// CRC-32 (IEEE): the fast accidental-corruption check, not a security
    /// primitive.
    Crc32,
}

impl HashAlgo {
    /// Lowercase name, as printed and accepted on the command line.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
            Self::Crc32 => "crc32",
        }
    }

    /// Every algorithm, in a stable order.
    pub const ALL: [Self; 3] = [Self::Sha256, Self::Sha512, Self::Crc32];

    /// Parse a name, accepting the common spellings.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "sha256" | "sha-256" => Some(Self::Sha256),
            "sha512" | "sha-512" => Some(Self::Sha512),
            "crc32" | "crc-32" => Some(Self::Crc32),
            _ => None,
        }
    }
}

impl std::fmt::Display for HashAlgo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// One file's digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDigest {
    /// The path that was read.
    pub path: PathBuf,
    /// Which algorithm produced the digest.
    pub algo: HashAlgo,
    /// Lowercase hex.
    pub hex: String,
    /// Bytes actually read, so a truncated read is visible rather than implied.
    pub bytes: u64,
}

/// Hash `path`, streaming it so a multi-gigabyte file never lands in memory.
///
/// A directory is refused rather than hashed: there is no single digest of a
/// tree, and inventing one would make the result incomparable with `sha256sum`.
pub fn hash_file(path: &Path, algo: HashAlgo) -> Result<FileDigest> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| Error::not_found(format!("{}: {e}", path.display())))?;
    if metadata.is_dir() {
        return Err(Error::invalid_input(format!(
            "{} is a directory; hash one file at a time",
            path.display()
        )));
    }

    let mut file = std::fs::File::open(path)
        .map_err(|e| Error::not_found(format!("{}: {e}", path.display())))?;
    let mut hasher = Hasher::new(algo);
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)
            .map_err(|e| Error::system(format!("{}: {e}", path.display())))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
    }

    Ok(FileDigest {
        path: path.to_path_buf(),
        algo,
        hex: hasher.finish(),
        bytes: total,
    })
}

/// Hash a byte slice. Used by the tests and by callers that already hold the
/// bytes.
pub fn hash_bytes(data: &[u8], algo: HashAlgo) -> String {
    let mut hasher = Hasher::new(algo);
    hasher.update(data);
    hasher.finish()
}

/// A streaming digest over one algorithm.
enum Hasher {
    Sha256(Sha256),
    Sha512(Sha512),
    Crc32(u32),
}

impl Hasher {
    fn new(algo: HashAlgo) -> Self {
        match algo {
            HashAlgo::Sha256 => Self::Sha256(Sha256::new()),
            HashAlgo::Sha512 => Self::Sha512(Sha512::new()),
            HashAlgo::Crc32 => Self::Crc32(0xFFFF_FFFF),
        }
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            Self::Sha256(inner) => inner.update(data),
            Self::Sha512(inner) => inner.update(data),
            Self::Crc32(state) => {
                for &byte in data {
                    *state ^= byte as u32;
                    for _ in 0..8 {
                        *state = if *state & 1 != 0 {
                            (*state >> 1) ^ 0xEDB8_8320
                        } else {
                            *state >> 1
                        };
                    }
                }
            }
        }
    }

    fn finish(self) -> String {
        match self {
            Self::Sha256(inner) => inner.finish(),
            Self::Sha512(inner) => inner.finish(),
            Self::Crc32(state) => format!("{:08x}", !state),
        }
    }
}

/// SHA-256 (FIPS 180-4).
struct Sha256 {
    state: [u32; 8],
    buffer: Vec<u8>,
    length: u64,
}

impl Sha256 {
    const K: [u32; 64] = [
        0x428a_2f98, 0x7137_4491, 0xb5c0_fbcf, 0xe9b5_dba5, 0x3956_c25b, 0x59f1_11f1,
        0x923f_82a4, 0xab1c_5ed5, 0xd807_aa98, 0x1283_5b01, 0x2431_85be, 0x550c_7dc3,
        0x72be_5d74, 0x80de_b1fe, 0x9bdc_06a7, 0xc19b_f174, 0xe49b_69c1, 0xefbe_4786,
        0x0fc1_9dc6, 0x240c_a1cc, 0x2de9_2c6f, 0x4a74_84aa, 0x5cb0_a9dc, 0x76f9_88da,
        0x983e_5152, 0xa831_c66d, 0xb003_27c8, 0xbf59_7fc7, 0xc6e0_0bf3, 0xd5a7_9147,
        0x06ca_6351, 0x1429_2967, 0x27b7_0a85, 0x2e1b_2138, 0x4d2c_6dfc, 0x5338_0d13,
        0x650a_7354, 0x766a_0abb, 0x81c2_c92e, 0x9272_2c85, 0xa2bf_e8a1, 0xa81a_664b,
        0xc24b_8b70, 0xc76c_51a3, 0xd192_e819, 0xd699_0624, 0xf40e_3585, 0x106a_a070,
        0x19a4_c116, 0x1e37_6c08, 0x2748_774c, 0x34b0_bcb5, 0x391c_0cb3, 0x4ed8_aa4a,
        0x5b9c_ca4f, 0x682e_6ff3, 0x748f_82ee, 0x78a5_636f, 0x84c8_7814, 0x8cc7_0208,
        0x90be_fffa, 0xa450_6ceb, 0xbef9_a3f7, 0xc671_78f2,
    ];

    fn new() -> Self {
        Self {
            state: [
                0x6a09_e667, 0xbb67_ae85, 0x3c6e_f372, 0xa54f_f53a, 0x510e_527f, 0x9b05_688c,
                0x1f83_d9ab, 0x5be0_cd19,
            ],
            buffer: Vec::new(),
            length: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        self.buffer.extend_from_slice(data);
        let full = self.buffer.len() / 64 * 64;
        if full > 0 {
            let mut chunk = self.buffer.drain(..full).collect::<Vec<u8>>();
            for block in chunk.chunks_exact_mut(64) {
                self.compress(block);
            }
        }
    }

    fn compress(&mut self, block: &[u8]) {
        let mut w = [0u32; 64];
        for (index, word) in block.chunks_exact(4).enumerate().take(16) {
            w[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for (index, &round_constant) in Self::K.iter().take(64).enumerate() {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(round_constant)
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        for (slot, value) in self
            .state
            .iter_mut()
            .zip([a, b, c, d, e, f, g, h])
        {
            *slot = slot.wrapping_add(value);
        }
    }

    fn finish(mut self) -> String {
        let bits = self.length.wrapping_mul(8);
        self.buffer.push(0x80);
        while self.buffer.len() % 64 != 56 {
            self.buffer.push(0);
        }
        let mut padded = std::mem::take(&mut self.buffer);
        padded.extend_from_slice(&bits.to_be_bytes());
        for block in padded.chunks_exact(64) {
            self.compress(block);
        }
        self.state.iter().map(|word| format!("{word:08x}")).collect()
    }
}

/// SHA-512 (FIPS 180-4).
struct Sha512 {
    state: [u64; 8],
    buffer: Vec<u8>,
    length: u128,
}

impl Sha512 {
    const K: [u64; 80] = [
        0x428a_2f98_d728_ae22, 0x7137_4491_23ef_65cd, 0xb5c0_fbcf_ec4d_3b2f, 0xe9b5_dba5_8189_dbbc,
        0x3956_c25b_f348_b538, 0x59f1_11f1_b605_d019, 0x923f_82a4_af19_4f9b, 0xab1c_5ed5_da6d_8118,
        0xd807_aa98_a303_0242, 0x1283_5b01_4570_6fbe, 0x2431_85be_4ee4_b28c, 0x550c_7dc3_d5ff_b4e2,
        0x72be_5d74_f27b_896f, 0x80de_b1fe_3b16_96b1, 0x9bdc_06a7_25c7_1235, 0xc19b_f174_cf69_2694,
        0xe49b_69c1_9ef1_4ad2, 0xefbe_4786_384f_25e3, 0x0fc1_9dc6_8b8c_d5b5, 0x240c_a1cc_77ac_9c65,
        0x2de9_2c6f_592b_0275, 0x4a74_84aa_6ea6_e483, 0x5cb0_a9dc_bd41_fbd4, 0x76f9_88da_8311_53b5,
        0x983e_5152_ee66_dfab, 0xa831_c66d_2db4_3210, 0xb003_27c8_98fb_213f, 0xbf59_7fc7_beef_0ee4,
        0xc6e0_0bf3_3da8_8fc2, 0xd5a7_9147_930a_a725, 0x06ca_6351_e003_826f, 0x1429_2967_0a0e_6e70,
        0x27b7_0a85_46d2_2ffc, 0x2e1b_2138_5c26_c926, 0x4d2c_6dfc_5ac4_2aed, 0x5338_0d13_9d95_b3df,
        0x650a_7354_8baf_63de, 0x766a_0abb_3c77_b2a8, 0x81c2_c92e_47ed_aee6, 0x9272_2c85_1482_353b,
        0xa2bf_e8a1_4cf1_0364, 0xa81a_664b_bc42_3001, 0xc24b_8b70_d0f8_9791, 0xc76c_51a3_0654_be30,
        0xd192_e819_d6ef_5218, 0xd699_0624_5565_a910, 0xf40e_3585_5771_202a, 0x106a_a070_32bb_d1b8,
        0x19a4_c116_b8d2_d0c8, 0x1e37_6c08_5141_ab53, 0x2748_774c_df8e_eb99, 0x34b0_bcb5_e19b_48a8,
        0x391c_0cb3_c5c9_5a63, 0x4ed8_aa4a_e341_8acb, 0x5b9c_ca4f_7763_e373, 0x682e_6ff3_d6b2_b8a3,
        0x748f_82ee_5def_b2fc, 0x78a5_636f_4317_2f60, 0x84c8_7814_a1f0_ab72, 0x8cc7_0208_1a64_39ec,
        0x90be_fffa_2363_1e28, 0xa450_6ceb_de82_bde9, 0xbef9_a3f7_b2c6_7915, 0xc671_78f2_e372_532b,
        0xca27_3ece_ea26_619c, 0xd186_b8c7_21c0_c207, 0xeada_7dd6_cde0_eb1e, 0xf57d_4f7f_ee6e_d178,
        0x06f0_67aa_7217_6fba, 0x0a63_7dc5_a2c8_98a6, 0x113f_9804_bef9_0dae, 0x1b71_0b35_131c_471b,
        0x28db_77f5_2304_7d84, 0x32ca_ab7b_40c7_2493, 0x3c9e_be0a_15c9_bebc, 0x431d_67c4_9c10_0d4c,
        0x4cc5_d4be_cb3e_42b6, 0x597f_299c_fc65_7e2a, 0x5fcb_6fab_3ad6_faec, 0x6c44_198c_4a47_5817,
    ];

    fn new() -> Self {
        Self {
            state: [
                0x6a09_e667_f3bc_c908,
                0xbb67_ae85_84ca_a73b,
                0x3c6e_f372_fe94_f82b,
                0xa54f_f53a_5f1d_36f1,
                0x510e_527f_ade6_82d1,
                0x9b05_688c_2b3e_6c1f,
                0x1f83_d9ab_fb41_bd6b,
                0x5be0_cd19_137e_2179,
            ],
            buffer: Vec::new(),
            length: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u128);
        self.buffer.extend_from_slice(data);
        let full = self.buffer.len() / 128 * 128;
        if full > 0 {
            let chunk = self.buffer.drain(..full).collect::<Vec<u8>>();
            for block in chunk.chunks_exact(128) {
                self.compress(block);
            }
        }
    }

    fn compress(&mut self, block: &[u8]) {
        let mut w = [0u64; 80];
        for (index, word) in block.chunks_exact(8).enumerate().take(16) {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(word);
            w[index] = u64::from_be_bytes(bytes);
        }
        for index in 16..80 {
            let s0 = w[index - 15].rotate_right(1)
                ^ w[index - 15].rotate_right(8)
                ^ (w[index - 15] >> 7);
            let s1 = w[index - 2].rotate_right(19)
                ^ w[index - 2].rotate_right(61)
                ^ (w[index - 2] >> 6);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for (index, &round_constant) in Self::K.iter().enumerate() {
            let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(round_constant)
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        for (slot, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }

    fn finish(mut self) -> String {
        let bits = self.length.wrapping_mul(8);
        self.buffer.push(0x80);
        while self.buffer.len() % 128 != 112 {
            self.buffer.push(0);
        }
        let mut padded = std::mem::take(&mut self.buffer);
        padded.extend_from_slice(&bits.to_be_bytes());
        for block in padded.chunks_exact(128) {
            self.compress(block);
        }
        self.state.iter().map(|word| format!("{word:016x}")).collect()
    }
}

fn recursive_size(path: &Path, metadata: &std::fs::Metadata) -> u64 {
    if !metadata.is_dir() {
        return metadata.len();
    }
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        total += if meta.is_dir() {
            recursive_size(&entry.path(), &meta)
        } else {
            meta.len()
        };
    }
    total
}

fn copy_path(from: &Path, to: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(from)
        .map_err(|e| Error::not_found(format!("{}: {e}", from.display())))?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)
            .map_err(|e| Error::system(format!("cannot create {}: {e}", to.display())))?;
        let entries = std::fs::read_dir(from)
            .map_err(|e| Error::system(format!("cannot read {}: {e}", from.display())))?;
        for entry in entries.flatten() {
            copy_path(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ()).map_err(|e| {
            Error::system(format!(
                "cannot copy {} -> {}: {e}",
                from.display(),
                to.display()
            ))
        })
    }
}

fn move_path(from: &Path, to: &Path) -> Result<()> {
    if let Err(rename_error) = std::fs::rename(from, to) {
        // Cross-device moves need a copy + delete.
        copy_path(from, to)?;
        remove_path(from).map_err(|_e| {
            Error::system(format!(
                "copied to {} but cannot remove source {}: {rename_error}",
                to.display(),
                from.display()
            ))
        })?;
    }
    Ok(())
}

fn remove_path(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| Error::not_found(format!("{}: {e}", path.display())))?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .map_err(|e| Error::system(format!("cannot remove {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("x-core-file-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn copy_and_move_a_tree() {
        let dir = temp_dir("copy-move");
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("nested")).unwrap();
        std::fs::write(src.join("nested/a.txt"), b"hello").unwrap();

        let copied = dir.join("dst");
        copy_path(&src, &copied).unwrap();
        assert_eq!(
            std::fs::read(copied.join("nested/a.txt")).unwrap(),
            b"hello"
        );

        move_path(&copied, &dir.join("renamed")).unwrap();
        assert!(!copied.exists());
        assert!(dir.join("renamed/nested/a.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn size_walks_directories() {
        let dir = temp_dir("size");
        std::fs::write(dir.join("a"), vec![0u8; 100]).unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/b"), vec![0u8; 30]).unwrap();
        assert_eq!(recursive_size(&dir, &std::fs::metadata(&dir).unwrap()), 130);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- digests -----------------------------------------------------------
    // The vectors below are the published ones for each algorithm. Hand-rolled
    // hashing is only trustworthy if it reproduces them exactly, so these are
    // pinned rather than self-consistent.

    #[test]
    fn sha256_matches_the_published_vectors() {
        assert_eq!(
            hash_bytes(b"abc", HashAlgo::Sha256),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hash_bytes(b"hello", HashAlgo::Sha256),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(
            hash_bytes(b"", HashAlgo::Sha256),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha512_matches_the_published_vectors() {
        assert_eq!(
            hash_bytes(b"abc", HashAlgo::Sha512),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(
            hash_bytes(b"", HashAlgo::Sha512),
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
             47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
        );
    }

    #[test]
    fn crc32_matches_its_standard_check_value() {
        // CRC-32/ISO-HDLC check value for "123456789".
        assert_eq!(hash_bytes(b"123456789", HashAlgo::Crc32), "cbf43926");
        assert_eq!(hash_bytes(b"", HashAlgo::Crc32), "00000000");
    }

    #[test]
    fn sha256_streams_correctly_across_block_boundaries() {
        // One million 'a' crosses many 64-byte blocks and the 55/56-byte
        // padding rule; it is the classic vector for exactly that.
        let data = vec![b'a'; 1_000_000];
        assert_eq!(
            hash_bytes(&data, HashAlgo::Sha256),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
        assert_eq!(
            hash_bytes(&data, HashAlgo::Sha512),
            "e718483d0ce769644e2e42c7bc15b4638e1f98b13b2044285632a803afa973eb\
             de0ff244877ea60a4cb0432ce577c31beb009c5c2c49aa2e4eadb217ad8cc09b"
        );
    }

    #[test]
    fn streaming_in_pieces_equals_hashing_at_once() {
        // The file path feeds the hasher in 64 KiB chunks; a digest that only
        // matched whole-buffer input would pass every vector above and still be
        // wrong for a real file.
        let data: Vec<u8> = (0..200_000u32).map(|index| index as u8).collect();
        let whole = hash_bytes(&data, HashAlgo::Sha256);
        let chunked = {
            let mut hasher = Hasher::new(HashAlgo::Sha256);
            for chunk in data.chunks(4096) {
                hasher.update(chunk);
            }
            hasher.finish()
        };
        assert_eq!(whole, chunked);
    }

    #[test]
    fn a_file_digest_matches_its_bytes_and_reports_the_length() {
        let dir = temp_dir("hash");
        let path = dir.join("payload.bin");
        std::fs::write(&path, b"abc").unwrap();

        let digest = hash_file(&path, HashAlgo::Sha256).expect("hash");
        assert_eq!(digest.algo, HashAlgo::Sha256);
        assert_eq!(digest.bytes, 3);
        assert_eq!(
            digest.hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(digest.path, path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_algorithm_agrees_on_an_empty_file() {
        let dir = temp_dir("hash-empty");
        let path = dir.join("empty");
        std::fs::write(&path, b"").unwrap();

        for algo in HashAlgo::ALL {
            let digest = hash_file(&path, algo).expect("hash");
            assert_eq!(digest.bytes, 0, "{algo}");
            assert!(!digest.hex.is_empty(), "{algo}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_is_refused_rather_than_hashed() {
        let dir = temp_dir("hash-dir");
        // There is no single digest of a tree that `sha256sum` would agree
        // with, so pretending otherwise would be a lie.
        let err = hash_file(&dir, HashAlgo::Sha256).expect_err("directories refused");
        assert_eq!(err.kind(), crate::error::ErrorKind::InvalidInput, "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_path_is_not_found() {
        let err = hash_file(Path::new("/definitely/not/here"), HashAlgo::Sha256)
            .expect_err("missing");
        assert_eq!(err.kind(), crate::error::ErrorKind::NotFound, "{err}");
    }

    #[test]
    fn algorithm_names_round_trip_and_accept_the_usual_spellings() {
        for algo in HashAlgo::ALL {
            assert_eq!(HashAlgo::parse(algo.name()), Some(algo));
            assert_eq!(HashAlgo::parse(&algo.name().to_uppercase()), Some(algo));
            assert_eq!(algo.to_string(), algo.name());
        }
        assert_eq!(HashAlgo::parse("sha-256"), Some(HashAlgo::Sha256));
        assert_eq!(HashAlgo::parse(" CRC-32 "), Some(HashAlgo::Crc32));
        assert_eq!(HashAlgo::parse("md5"), None, "unsupported stays unsupported");
        assert_eq!(HashAlgo::default(), HashAlgo::Sha256);
    }
}
