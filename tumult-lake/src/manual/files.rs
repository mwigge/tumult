//! The files a record keeps as evidence (schema v22).
//!
//! A file is stored once, in the `attachments` folder next to the lake
//! database, at `attachments/<first 2 hex>/<sha256>`. It is written to a
//! temporary file in that folder first and renamed into place. A file that
//! is there already is never written again: the same hash is the same
//! bytes. Its `evidence_attachments` row keeps the hash, the media type,
//! the size and the original name as the label.
//!
//! The media type is read from the bytes, never from what the client
//! says: PDF, PNG and JPEG by their first bytes, and plain text, CSV and
//! JSON when the bytes are UTF-8 text, the name choosing among the three.
//! A name that says another type than the bytes are is refused, and so is
//! anything else.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::hash::ulid;
use crate::StoreError;

/// The largest file a record takes: 25 MB.
pub const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;

/// The media types a file may have, in the order an error lists them.
pub const MEDIA_TYPES: [&str; 6] = [
    "application/pdf",
    "image/png",
    "image/jpeg",
    "text/plain",
    "text/csv",
    "application/json",
];

/// The longest original name a file may have, in characters.
const MAX_NAME_CHARS: usize = 255;

/// The first bytes of each binary type, and its media type.
const MAGIC: [(&[u8], &str); 3] = [
    (b"%PDF-", "application/pdf"),
    (b"\x89PNG\r\n\x1a\n", "image/png"),
    (b"\xff\xd8\xff", "image/jpeg"),
];

/// The media type a file name says it has, by its extension.
fn named_type(name: &str) -> Option<&'static str> {
    let (_, ext) = name.rsplit_once('.')?;
    Some(match ext.to_ascii_lowercase().as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        _ => return None,
    })
}

/// The media type of a file that starts with `head` and is UTF-8 text when
/// `text` is set, stored as `name`.
fn media_type(head: &[u8], text: bool, name: &str) -> Result<&'static str, FileError> {
    let named = named_type(name);
    let found = MAGIC
        .iter()
        .find(|(magic, _)| head.starts_with(magic))
        .map(|(_, media)| *media);
    let found = match (found, named) {
        (Some(found), _) => found,
        (None, Some(named @ ("text/plain" | "text/csv" | "application/json"))) if text => named,
        (None, None) if text => "text/plain",
        (None, _) => {
            return Err(FileError::Unsupported(
                "the bytes are not a PDF, a PNG, a JPEG or UTF-8 text".into(),
            ))
        }
    };
    match named {
        Some(named) if named != found => Err(FileError::Unsupported(format!(
            "the name says {named}, but the bytes are {found}"
        ))),
        _ => Ok(found),
    }
}

/// The `attachments` folder next to the lake database at `db_path`, as the
/// lake export folder is found next to it.
#[must_use]
pub fn attachments_dir(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("attachments")
}

/// Whether `hash` is a SHA-256 in lower-case hex.
fn is_sha256(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The place under `dir` of the stored file with SHA-256 `hash`; `None`
/// when `hash` is not one, so no path outside `dir` is ever made.
#[must_use]
pub fn stored_file(dir: &Path, hash: &str) -> Option<PathBuf> {
    is_sha256(hash).then(|| dir.join(&hash[..2]).join(hash))
}

/// Why a file is not taken.
#[derive(Debug)]
pub enum FileError {
    /// The file is larger than [`MAX_FILE_BYTES`].
    TooLarge,
    /// The bytes are none of [`MEDIA_TYPES`], or the name says another
    /// type than the bytes are: the message says which.
    Unsupported(String),
    /// The file is empty, or its name is empty or too long.
    Invalid(String),
    Io(std::io::Error),
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => write!(f, "the file is larger than 25 MB ({MAX_FILE_BYTES} bytes)"),
            Self::Unsupported(msg) => write!(
                f,
                "the file type is not allowed: {msg} (allowed: {})",
                MEDIA_TYPES.join(", ")
            ),
            Self::Invalid(msg) => write!(f, "{msg}"),
            Self::Io(e) => write!(f, "the file could not be stored: {e}"),
        }
    }
}

impl std::error::Error for FileError {}

impl From<std::io::Error> for FileError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// `digest` in lower-case hex.
fn hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Create `dir` and the folders above it, readable by the owner only.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Open `path` as a new file the owner only can read; never an existing one.
fn private_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Remove `path`, which may already be gone; a failure is logged, since
/// the caller is cleaning up after another outcome.
fn discard(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "could not remove a temporary file")
        }
    }
}

/// Whether the bytes seen so far are UTF-8 text with no NUL byte. A
/// character split between two writes is carried to the next one.
#[derive(Default)]
struct TextCheck {
    carry: Vec<u8>,
    broken: bool,
}

impl TextCheck {
    fn feed(&mut self, bytes: &[u8]) {
        if self.broken {
            return;
        }
        if bytes.contains(&0) {
            self.broken = true;
            return;
        }
        let joined;
        let data = if self.carry.is_empty() {
            bytes
        } else {
            joined = [self.carry.as_slice(), bytes].concat();
            &joined
        };
        self.carry = match std::str::from_utf8(data) {
            Ok(_) => Vec::new(),
            Err(e) if e.error_len().is_none() => data[e.valid_up_to()..].to_vec(),
            Err(_) => {
                self.broken = true;
                Vec::new()
            }
        };
    }

    fn is_text(&self) -> bool {
        !self.broken && self.carry.is_empty()
    }
}

/// A file being received, written to a temporary file in the attachments
/// folder as it arrives. Dropped before [`FileUpload::finish`], it removes
/// what it wrote.
pub struct FileUpload {
    dir: PathBuf,
    temp: PathBuf,
    file: File,
    hasher: Sha256,
    size: u64,
    /// The first bytes, to read the type from.
    head: Vec<u8>,
    text: TextCheck,
}

impl FileUpload {
    /// Start a file in the attachments folder `dir`, made if needed.
    ///
    /// # Errors
    /// Returns `Io` when the folder or the temporary file cannot be made.
    pub fn begin(dir: &Path) -> Result<Self, FileError> {
        private_dir(dir)?;
        let temp = dir.join(format!(".upload-{}", ulid()));
        let file = private_file(&temp)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            temp,
            file,
            hasher: Sha256::new(),
            size: 0,
            head: Vec::with_capacity(8),
            text: TextCheck::default(),
        })
    }

    /// Add `bytes` to the file. Nothing past [`MAX_FILE_BYTES`] is written.
    ///
    /// # Errors
    /// Returns `TooLarge` when the file would grow past the cap, `Io` when
    /// the write fails.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), FileError> {
        let size = self.size + bytes.len() as u64;
        if size > MAX_FILE_BYTES {
            return Err(FileError::TooLarge);
        }
        self.file.write_all(bytes)?;
        self.hasher.update(bytes);
        let room = 8usize.saturating_sub(self.head.len()).min(bytes.len());
        self.head.extend_from_slice(&bytes[..room]);
        self.text.feed(bytes);
        self.size = size;
        Ok(())
    }

    /// End the file and decide its media type. `name` is the original
    /// name: its last part, after any `/` or `\`, is kept as the label.
    ///
    /// # Errors
    /// Returns `Invalid` for an empty file or an empty or overlong name,
    /// `Unsupported` for a type that is not allowed, `Io` when the file
    /// cannot be flushed to disk.
    pub fn finish(mut self, name: &str) -> Result<StagedFile, FileError> {
        let name = label_of(name)?;
        if self.size == 0 {
            return Err(FileError::Invalid("the file is empty".into()));
        }
        let media_type = media_type(&self.head, self.text.is_text(), &name)?;
        self.file.sync_all()?;
        let hash = hex(&std::mem::take(&mut self.hasher).finalize());
        Ok(StagedFile {
            dir: self.dir.clone(),
            temp: Some(std::mem::take(&mut self.temp)),
            hash,
            media_type,
            size_bytes: self.size,
            name,
        })
    }
}

impl Drop for FileUpload {
    fn drop(&mut self) {
        // Empty once `finish` has handed the file on.
        if !self.temp.as_os_str().is_empty() {
            discard(&self.temp);
        }
    }
}

/// The label of a file sent as `name`: its last part, with no control
/// characters.
fn label_of(name: &str) -> Result<String, FileError> {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let label: String = last.chars().filter(|c| !c.is_control()).collect();
    let label = label.trim();
    if label.is_empty() {
        return Err(FileError::Invalid("the file has no name".into()));
    }
    if label.chars().count() > MAX_NAME_CHARS {
        return Err(FileError::Invalid(format!(
            "the file name is longer than {MAX_NAME_CHARS} characters"
        )));
    }
    Ok(label.to_string())
}

/// A received file whose type is known, waiting in its temporary file to
/// be stored with its row. Dropped before it is stored, it removes the
/// temporary file.
#[derive(Debug)]
pub struct StagedFile {
    dir: PathBuf,
    temp: Option<PathBuf>,
    hash: String,
    media_type: &'static str,
    size_bytes: u64,
    name: String,
}

/// Where [`StagedFile::place`] put a file, and whether it was not there
/// before.
pub(crate) struct Placed {
    pub(crate) path: PathBuf,
    pub(crate) new: bool,
}

impl StagedFile {
    /// The SHA-256 of the bytes, in lower-case hex.
    #[must_use]
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// One of [`MEDIA_TYPES`].
    #[must_use]
    pub fn media_type(&self) -> &'static str {
        self.media_type
    }

    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// The original name, kept as the label.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Move the file to its place by its hash. A file that is there already
    /// is the same bytes and is never written again.
    pub(crate) fn place(&mut self) -> std::io::Result<Placed> {
        let parent = self.dir.join(&self.hash[..2]);
        let path = parent.join(&self.hash);
        let Some(temp) = self.temp.take() else {
            return Ok(Placed { path, new: false });
        };
        let moved = move_into(&temp, &parent, &path);
        if !matches!(moved, Ok(true)) {
            discard(&temp);
        }
        moved.map(|new| Placed { path, new })
    }
}

/// Rename `temp` to `path` in `parent` unless `path` is there already;
/// returns whether it was renamed.
fn move_into(temp: &Path, parent: &Path, path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    private_dir(parent)?;
    std::fs::rename(temp, path)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(true)
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        if let Some(temp) = &self.temp {
            discard(temp);
        }
    }
}

/// Copy `from` to `to` through a temporary file next to `to`, and keep it
/// only when its bytes hash to `hash`. Returns whether the copy was kept.
///
/// # Errors
/// Returns an error when `from` cannot be read or `to` cannot be written.
pub(crate) fn copy_verified(from: &Path, to: &Path, hash: &str) -> Result<bool, StoreError> {
    let parent = to
        .parent()
        .ok_or_else(|| StoreError::Internal("a stored file has no folder".into()))?;
    private_dir(parent)?;
    let temp = parent.join(format!(".copy-{}", ulid()));
    let mut out = private_file(&temp)?;
    let copied = (|| {
        let mut input = File::open(from)?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let n = std::io::Read::read(&mut input, &mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            out.write_all(&buffer[..n])?;
        }
        out.sync_all()?;
        Ok::<_, std::io::Error>(hex(&hasher.finalize()))
    })();
    drop(out);
    match copied {
        Ok(found) if found == hash => {
            std::fs::rename(&temp, to)?;
            #[cfg(unix)]
            File::open(parent)?.sync_all()?;
            Ok(true)
        }
        Ok(_) => {
            discard(&temp);
            Ok(false)
        }
        Err(e) => {
            discard(&temp);
            Err(e.into())
        }
    }
}

/// Whether the file at `path` is there and hashes to `hash`.
///
/// # Errors
/// Returns an error when the file is there but cannot be read.
fn holds(path: &Path, hash: &str) -> Result<bool, StoreError> {
    if !path.is_file() {
        return Ok(false);
    }
    Ok(crate::file_integrity::checksum(path)? == hash)
}

/// The hashes of the stored files the attachment rows name.
pub(crate) const FILE_HASHES_SQL: &str = "SELECT DISTINCT file_hash AS h \
     FROM evidence_attachments WHERE kind = 'file' AND file_hash IS NOT NULL ORDER BY h";

/// Where the file with SHA-256 `hash` is kept in a lake export or a
/// backup, relative to its folder.
fn copy_path(hash: &str) -> String {
    format!("attachments/{}/{hash}", &hash[..2])
}

/// Copy the stored file of each of `hashes` from the attachments folder
/// `files` into `dir`, where it is not there with its bytes already.
/// Returns the files `dir` holds, each SHA-256 with its path in `dir`, and
/// the hashes it cannot hold: a stored file that is missing or changed is
/// logged and named, and the copy goes on.
///
/// # Errors
/// Returns an error when a file cannot be read or written.
pub(crate) fn carry(
    files: &Path,
    dir: &Path,
    hashes: &[String],
) -> Result<(BTreeMap<String, String>, Vec<String>), StoreError> {
    let mut carried = BTreeMap::new();
    let mut missing = Vec::new();
    for hash in hashes {
        let Some(from) = stored_file(files, hash) else {
            tracing::error!(file_hash = %hash, "an attachment row names no SHA-256; no file is copied for it");
            missing.push(hash.clone());
            continue;
        };
        let rel = copy_path(hash);
        let to = dir.join(&rel);
        let held = holds(&to, hash)? || (from.is_file() && copy_verified(&from, &to, hash)?);
        if !held {
            tracing::error!(file_hash = %hash, "the stored file of an attachment is missing or changed; it is not copied");
            missing.push(hash.clone());
            continue;
        }
        carried.insert(hash.clone(), rel);
    }
    Ok((carried, missing))
}

/// Bring the files `listed` in `dir` (each SHA-256 with its path in `dir`)
/// into the attachments folder `files`. A file that is there already is
/// never written again. Returns the files copied.
///
/// # Errors
/// Rejects an entry that is not a SHA-256 at its path, and a file of `dir`
/// that is missing or does not hash to its name. The files copied before
/// stay, and a second run goes on from there.
pub(crate) fn bring_back(
    dir: &Path,
    listed: &BTreeMap<String, String>,
    files: &Path,
) -> Result<usize, StoreError> {
    let mut copied = 0;
    for (hash, rel) in listed {
        let to = stored_file(files, hash)
            .filter(|_| *rel == copy_path(hash))
            .ok_or_else(|| StoreError::Internal("invalid attachment manifest path".into()))?;
        if to.symlink_metadata().is_ok() {
            continue;
        }
        let from = dir.join(rel);
        if !from.is_file() || !copy_verified(&from, &to, hash)? {
            return Err(StoreError::Internal(format!(
                "attachment file {hash} is missing or does not hash to its name"
            )));
        }
        copied += 1;
    }
    Ok(copied)
}

/// Remove the temporary files of uploads in the attachments folder `files`
/// that were last written more than `age` ago: uploads cut off by a stop
/// or a crash. Returns how many were removed.
///
/// # Errors
/// Returns an error when the folder cannot be read or a file removed.
pub(crate) fn sweep_uploads(files: &Path, age: Duration) -> std::io::Result<usize> {
    let entries = match std::fs::read_dir(files) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with(".upload-") {
            continue;
        }
        let meta = entry.metadata()?;
        let stale = meta.modified()?.elapsed().is_ok_and(|since| since > age);
        if meta.is_file() && stale {
            std::fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
    const JPEG: &[u8] = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00";
    const PDF: &[u8] = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n";
    const ELF: &[u8] = b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x02\x00\x3e\x00";

    /// The media type of `bytes` sent as `name`, written in two pieces
    /// split at `split`.
    fn type_of(bytes: &[u8], name: &str, split: usize) -> Result<&'static str, String> {
        let dir = tempfile::tempdir().unwrap();
        let mut upload = FileUpload::begin(dir.path()).unwrap();
        let split = split.min(bytes.len());
        upload.write(&bytes[..split]).unwrap();
        upload.write(&bytes[split..]).unwrap();
        upload
            .finish(name)
            .map(|staged| staged.media_type())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn the_media_type_is_read_from_the_bytes_and_the_name_must_not_say_another() {
        for (bytes, name, want) in [
            (PDF, "runbook.pdf", "application/pdf"),
            (PDF, "runbook", "application/pdf"),
            (PNG, "screen.PNG", "image/png"),
            (JPEG, "photo.jpeg", "image/jpeg"),
            (JPEG, "photo.jpg", "image/jpeg"),
            (&b"restored 1204 orders\n"[..], "restore.log", "text/plain"),
            (&b"restored 1204 orders\n"[..], "restore", "text/plain"),
            (&b"a,b\n1,2\n"[..], "orders.CSV", "text/csv"),
            (
                &b"{\"orders\": 1204}"[..],
                "orders.json",
                "application/json",
            ),
            ("återställd på 40 s\n".as_bytes(), "notes.txt", "text/plain"),
        ] {
            // Split inside the first character too, so a character cut in
            // two between writes still counts as text.
            for split in [1, 3, bytes.len()] {
                assert_eq!(
                    type_of(bytes, name, split),
                    Ok(want),
                    "{name} split at {split}"
                );
            }
        }
        for (bytes, name, why) in [
            (ELF, "runbook.pdf", "not a PDF, a PNG, a JPEG or UTF-8 text"),
            (ELF, "tool", "not a PDF, a PNG, a JPEG or UTF-8 text"),
            (
                PDF,
                "screen.png",
                "the name says image/png, but the bytes are application/pdf",
            ),
            (&b"plain words"[..], "runbook.pdf", "not a PDF"),
            (
                PNG,
                "notes.txt",
                "the name says text/plain, but the bytes are image/png",
            ),
            (&b"text\x00with a nul"[..], "notes.txt", "not a PDF"),
            (&b"\xc3\x28 broken"[..], "notes.txt", "not a PDF"),
        ] {
            let err = type_of(bytes, name, 2).unwrap_err();
            assert!(err.contains(why), "{name}: {err}");
            assert!(err.contains("allowed: application/pdf, image/png"), "{err}");
        }
    }

    #[test]
    fn the_label_is_the_last_part_of_the_name_and_an_empty_file_is_refused() {
        assert_eq!(
            type_of(PDF, "../../etc/runbook.pdf", 2),
            Ok("application/pdf")
        );
        let dir = tempfile::tempdir().unwrap();
        let mut upload = FileUpload::begin(dir.path()).unwrap();
        upload.write(PDF).unwrap();
        let staged = upload.finish("C:\\logs\\run\nbook.pdf").unwrap();
        assert_eq!(staged.name(), "runbook.pdf");
        drop(staged);
        for name in ["", "dir/", "\u{7}"] {
            let mut upload = FileUpload::begin(dir.path()).unwrap();
            upload.write(PDF).unwrap();
            assert!(
                matches!(upload.finish(name), Err(FileError::Invalid(_))),
                "{name:?}"
            );
        }
        let long = format!("{}.pdf", "a".repeat(MAX_NAME_CHARS));
        let mut upload = FileUpload::begin(dir.path()).unwrap();
        upload.write(PDF).unwrap();
        assert!(matches!(upload.finish(&long), Err(FileError::Invalid(_))));
        let upload = FileUpload::begin(dir.path()).unwrap();
        let err = upload.finish("empty.txt").unwrap_err();
        assert_eq!(err.to_string(), "the file is empty");
        // Every refused or dropped upload left nothing behind.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_stored_file_is_found_only_by_a_sha256() {
        let dir = Path::new("lake").join("attachments");
        let hash = "ab".repeat(32);
        assert_eq!(stored_file(&dir, &hash), Some(dir.join("ab").join(&hash)));
        for bad in [
            "../../../etc/passwd",
            &"AB".repeat(32),
            &"ab".repeat(31),
            "",
        ] {
            assert_eq!(stored_file(&dir, bad), None, "{bad}");
        }
        assert_eq!(
            attachments_dir(&Path::new("lake").join("store.duckdb")),
            dir
        );
    }
}
