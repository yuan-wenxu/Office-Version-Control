use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use similar::{ChangeTag, TextDiff};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

use crate::office::{
    detect_office_kind, equivalent_package_xml, extract_docx_parts, extract_package_text, extract_pptx_parts,
    is_supported_office_file,
};

type ImageEntries = BTreeMap<String, (String, Option<u64>)>;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedRecord {
    pub source_path: String,
    pub original_name: String,
    pub kind: String,
    pub hash: String,
    pub staged_at: String,
    pub size: u64,
    #[serde(default)]
    pub snapshot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionRecord {
    pub id: String,
    pub source_path: String,
    pub original_name: String,
    pub kind: String,
    pub hash: String,
    pub object_path: String,
    pub storage_mode: String,
    pub manifest_path: String,
    pub text_path: String,
    pub message: String,
    pub created_at: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryStatus {
    pub staged: Vec<StagedRecord>,
    pub modified: Vec<ModifiedRecord>,
    pub missing: Vec<VersionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModifiedRecord {
    pub source_path: String,
    pub original_name: String,
    pub previous_hash: String,
    pub current_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedFile {
    pub source_path: String,
    pub original_name: String,
    pub kind: String,
    pub versions: usize,
    pub latest_hash: String,
    pub latest_created_at: String,
    pub size: u64,
    pub staged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffVersionInfo {
    pub id: String,
    pub message: String,
    pub created_at: String,
    pub hash: String,
}

impl From<&VersionRecord> for DiffVersionInfo {
    fn from(record: &VersionRecord) -> Self {
        Self {
            id: record.id.clone(),
            message: record.message.clone(),
            created_at: record.created_at.clone(),
            hash: record.hash.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: String,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffImage {
    pub kind: String,
    pub path: String,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
    pub old_size: Option<u64>,
    pub new_size: Option<u64>,
    pub old_data_url: Option<String>,
    pub new_data_url: Option<String>,
    pub preview_supported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffResult {
    pub from: DiffVersionInfo,
    pub to: DiffVersionInfo,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub unchanged_lines: usize,
    pub lines: Vec<DiffLine>,
    pub added_images: usize,
    pub removed_images: usize,
    pub modified_images: usize,
    pub unchanged_images: usize,
    pub images: Vec<DiffImage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package_changes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryMetadata {
    schema_version: u8,
    files: HashMap<String, Vec<VersionRecord>>,
    staged: HashMap<String, StagedRecord>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pending: HashMap<String, StagedRecord>,
}

impl Default for RepositoryMetadata {
    fn default() -> Self {
        Self {
            schema_version: 1,
            files: HashMap::new(),
            staged: HashMap::new(),
            pending: HashMap::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageManifest {
    schema_version: u8,
    source_hash: String,
    original_name: String,
    created_at: String,
    entries: Vec<PackageManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageManifestEntry {
    name: String,
    dir: bool,
    hash: Option<String>,
    size: Option<u64>,
    #[serde(default)]
    compressed: bool,
}

#[derive(Debug, Clone)]
pub struct VersionManager {
    root: PathBuf,
}

impl VersionManager {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn init(&self) -> Result<()> {
        let is_new = !self.root.exists();
        fs::create_dir_all(self.objects())?;
        fs::create_dir_all(self.blobs())?;
        fs::create_dir_all(self.packages())?;
        fs::create_dir_all(self.snapshots())?;
        if is_new {
            try_set_hidden(&self.root);
        }
        if !self.metadata_path().exists() {
            self.write_metadata(&RepositoryMetadata::default())?;
        }
        Ok(())
    }

    pub fn add(&self, file_path: impl AsRef<Path>) -> Result<StagedRecord> {
        self.init()?;
        let path = normalize_existing_path(file_path.as_ref())?;
        let stat = validate_office_file(&path)?;
        let hash = sha256_file(&path)?;
        let source_path = self.source_key(&path)?;
        let record = StagedRecord {
            source_path,
            original_name: file_name(&path)?,
            kind: detect_office_kind(&path)?.to_string(),
            hash,
            staged_at: now()?,
            size: stat.len(),
            snapshot: false,
        };

        let mut metadata = self.read_metadata()?;
        metadata.pending.remove(&record.source_path);
        metadata.staged.insert(record.source_path.clone(), record.clone());
        self.write_metadata(&metadata)?;
        Ok(record)
    }

    pub fn stage(&self, file_path: &str) -> Result<StagedRecord> {
        let (mut staged, bytes) = self.read_working_file(file_path)?;
        self.store_snapshot(&staged, bytes)?;
        staged.snapshot = true;
        let mut metadata = self.read_metadata()?;
        metadata.pending.remove(&staged.source_path);
        metadata.staged.insert(staged.source_path.clone(), staged.clone());
        self.write_metadata(&metadata)?;
        Ok(staged)
    }

    pub fn unstage(&self, file_path: &str) -> Result<()> {
        let key = self.source_key_for_input(file_path)?;
        let mut metadata = self.read_metadata()?;
        if let Some(staged) = metadata.staged.remove(&key) {
            if !metadata.files.contains_key(&key) {
                metadata.pending.insert(key, staged);
            }
            self.write_metadata(&metadata)?;
        }
        Ok(())
    }

    #[allow(dead_code, reason = "The Office plug-in uses this operation through the local API.")]
    pub fn commit_file(&self, file_path: &str, message: &str) -> Result<Option<VersionRecord>> {
        let message = message.trim();
        if message.is_empty() {
            return Err("Commit message is required.".into());
        }
        let (snapshot, bytes) = self.read_working_file(file_path)?;
        self.store_snapshot(&snapshot, bytes)?;
        let mut metadata = self.read_metadata()?;
        let Some(record) = self.record_version(&mut metadata, &snapshot, message)? else {
            return Ok(None);
        };
        metadata.staged.remove(&snapshot.source_path);
        self.write_metadata(&metadata)?;
        Ok(Some(record))
    }

    pub fn commit_staged(&self, message: String) -> Result<Vec<VersionRecord>> {
        let message = message.trim();
        if message.is_empty() {
            return Err("Commit message is required.".into());
        }
        let mut metadata = self.read_metadata()?;
        if metadata.staged.is_empty() {
            return Err("Stage a file before committing.".into());
        }

        let mut staged = metadata.staged.values().cloned().collect::<Vec<_>>();
        staged.sort_by(|a, b| a.source_path.cmp(&b.source_path));
        let mut committed = Vec::new();
        for mut item in staged {
            if !item.snapshot {
                // Entries created by earlier versions only recorded a file path.
                let (current, bytes) = self.read_working_file(&item.source_path)?;
                self.store_snapshot(&current, bytes)?;
                item = current;
            }
            if let Some(version) = self.record_version(&mut metadata, &item, message)? {
                committed.push(version);
            }
        }
        metadata.staged.clear();
        self.write_metadata(&metadata)?;
        Ok(committed)
    }

    pub fn commit_tracked_changes(&self, message: String) -> Result<Vec<VersionRecord>> {
        self.init()?;
        let message = message.trim().to_string();
        if message.is_empty() {
            return Err("Commit message is required.".into());
        }

        let status = self.status()?;
        let mut files = BTreeMap::new();
        for item in status.staged {
            files.insert(item.source_path.clone(), item.source_path);
        }
        for item in status.modified {
            files.insert(item.source_path.clone(), item.source_path);
        }

        if files.is_empty() {
            return Err(
                "Nothing to commit. Add a new Office file or modify a tracked file first.".into(),
            );
        }

        let mut committed = Vec::new();
        for file in files.values() {
            committed.push(self.save_version(file, &message)?);
        }

        let mut metadata = self.read_metadata()?;
        for file in files.keys() {
            metadata.staged.remove(file);
        }
        self.write_metadata(&metadata)?;
        Ok(committed)
    }

    pub fn status(&self) -> Result<RepositoryStatus> {
        self.init()?;
        let metadata = self.read_metadata()?;
        let mut modified = Vec::new();
        let mut missing = Vec::new();

        for (source_path, versions) in &metadata.files {
            let Some(latest) = versions.last() else {
                continue;
            };
            let path = self.resolve_source_path(source_path);
            let Some(current_hash) = working_hash(&path)? else {
                missing.push(latest.clone());
                continue;
            };
            if current_hash != latest.hash || metadata.staged.get(source_path)
                .is_some_and(|staged| staged.snapshot && staged.hash != current_hash)
            {
                modified.push(ModifiedRecord {
                    source_path: source_path.clone(),
                    original_name: latest.original_name.clone(),
                    previous_hash: latest.hash.clone(),
                    current_hash,
                });
            }
        }

        for staged in metadata.staged.values().filter(|staged| {
            staged.snapshot && !metadata.files.contains_key(&staged.source_path)
        }) {
            let path = self.resolve_source_path(&staged.source_path);
            if let Some(current_hash) = working_hash(&path)? {
                if current_hash != staged.hash {
                    modified.push(ModifiedRecord {
                        source_path: staged.source_path.clone(),
                        original_name: staged.original_name.clone(),
                        previous_hash: staged.hash.clone(),
                        current_hash,
                    });
                }
            }
        }
        for pending in metadata.pending.values() {
            let path = self.resolve_source_path(&pending.source_path);
            if let Some(current_hash) = working_hash(&path)? {
                modified.push(ModifiedRecord {
                    source_path: pending.source_path.clone(),
                    original_name: pending.original_name.clone(),
                    previous_hash: String::new(),
                    current_hash,
                });
            }
        }
        Ok(RepositoryStatus {
            staged: metadata.staged.values().cloned().collect(),
            modified,
            missing,
        })
    }

    pub fn log(&self, file_path: Option<&str>) -> Result<Vec<VersionRecord>> {
        self.init()?;
        let metadata = self.read_metadata()?;
        if let Some(file_path) = file_path.filter(|value| !value.trim().is_empty()) {
            let key = self.source_key_for_input(file_path)?;
            return Ok(metadata.files.get(&key).cloned().unwrap_or_default());
        }

        let mut records: Vec<_> = metadata.files.values().flatten().cloned().collect();
        records.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(records)
    }

    pub fn tracked_files(&self) -> Result<Vec<TrackedFile>> {
        self.init()?;
        let metadata = self.read_metadata()?;
        let mut files = BTreeMap::new();

        for (source_path, records) in &metadata.files {
            let Some(latest) = records.last() else {
                continue;
            };
            files.insert(
                source_path.clone(),
                TrackedFile {
                    source_path: source_path.clone(),
                    original_name: latest.original_name.clone(),
                    kind: latest.kind.clone(),
                    versions: records.len(),
                    latest_hash: latest.hash.clone(),
                    latest_created_at: latest.created_at.clone(),
                    size: latest.size,
                    staged: metadata.staged.contains_key(source_path),
                },
            );
        }

        for (is_staged, entries) in [(true, &metadata.staged), (false, &metadata.pending)] {
            for (source_path, item) in entries {
                files.entry(source_path.clone()).or_insert_with(|| TrackedFile {
                    source_path: source_path.clone(),
                    original_name: item.original_name.clone(),
                    kind: item.kind.clone(),
                    versions: 0,
                    latest_hash: item.hash.clone(),
                    latest_created_at: item.staged_at.clone(),
                    size: item.size,
                    staged: is_staged,
                });
            }
        }

        Ok(files.into_values().collect())
    }

    pub fn diff_staged(&self, file_path: &str) -> Result<DiffResult> {
        let key = self.source_key_for_input(file_path)?;
        let metadata = self.read_metadata()?;
        let staged = metadata.staged.get(&key).ok_or("File is not staged.")?;
        if !staged.snapshot {
            return self.diff_working(file_path);
        }
        let to = self.snapshot_record(staged, "Staged".into(), "Staged changes")?;
        let from = metadata.files.get(&key).and_then(|versions| versions.last());
        self.diff_records(from, &to, true)
    }

    #[allow(dead_code, reason = "The desktop AI workflow uses this method; the CLI does not.")]
    pub fn staged_diffs(&self, previews: bool, limit: usize) -> Result<(usize, Vec<(String, DiffResult)>)> {
        let metadata = self.read_metadata()?;
        let mut staged = metadata.staged.values().collect::<Vec<_>>();
        staged.sort_by(|a, b| a.source_path.cmp(&b.source_path));
        if staged.is_empty() {
            return Err("Stage files before generating a commit message.".into());
        }
        let count = staged.len();
        let mut diffs = Vec::new();
        for entry in staged.into_iter().take(limit) {
            if !entry.snapshot {
                return Err(format!("Unstage and stage {} again to capture its contents.", entry.source_path).into());
            }
            let to = self.snapshot_record(entry, "Staged".into(), "Staged changes")?;
            let from = metadata.files.get(&entry.source_path).and_then(|versions| versions.last());
            diffs.push((entry.source_path.clone(), self.diff_records(from, &to, previews)?));
        }
        Ok((count, diffs))
    }

    pub fn diff_working(&self, file_path: &str) -> Result<DiffResult> {
        self.diff_working_copy(file_path, false)
    }

    pub fn diff_unstaged(&self, file_path: &str) -> Result<DiffResult> {
        self.diff_working_copy(file_path, true)
    }

    #[allow(dead_code, reason = "Used by the desktop change list.")]
    pub fn has_visible_change(
        &self,
        file_path: &str,
        base_hash: &str,
        current_hash: &str,
    ) -> Result<Option<bool>> {
        let metadata = self.read_metadata()?;
        let base = metadata.staged.get(file_path)
            .filter(|item| item.snapshot)
            .map(|item| (item.hash.as_str(), self.snapshot_paths(&item.hash).0))
            .or_else(|| {
                let latest = metadata.files.get(file_path)?.last()?;
                Some((latest.hash.as_str(), self.resolve_stored_path(&latest.manifest_path)))
            });
        let Some((hash, manifest_path)) = base else { return Ok(None) };
        if hash != base_hash {
            return Ok(None);
        }

        let bytes = match fs::read(self.resolve_source_path(file_path)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if sha256_bytes(&bytes) != current_hash {
            return Ok(None);
        }

        let manifest: PackageManifest = serde_json::from_slice(&fs::read(manifest_path)?)?;
        let mut archive = ZipArchive::new(Cursor::new(bytes))?;
        Ok(Some(!self.working_package_changes(&manifest, &mut archive, 1)?.is_empty()))
    }

    fn working_package_changes(
        &self,
        manifest: &PackageManifest,
        archive: &mut ZipArchive<Cursor<Vec<u8>>>,
        limit: usize,
    ) -> Result<Vec<String>> {
        let mut entries = manifest.entries.iter().filter(|entry| {
            !entry.dir && !is_save_metadata(&entry.name)
        }).map(|entry| (entry.name.as_str(), entry)).collect::<BTreeMap<_, _>>();
        let mut changes = Vec::new();

        for index in 0..archive.len() {
            let mut current = archive.by_index(index)?;
            let name = current.name().to_string();
            if current.is_dir() || is_save_metadata(&name) {
                continue;
            }
            if let Some(old) = entries.remove(name.as_str()) {
                let hash = old.hash.as_deref().ok_or("Missing package entry hash")?;
                let mut data = Vec::new();
                current.read_to_end(&mut data)?;
                if sha256_bytes(&data) == hash {
                    continue;
                }
                if name.ends_with(".xml") || name.ends_with(".rels") {
                    let previous = if old.compressed {
                        read_blob_data(&self.blobs(), hash)?
                    } else {
                        fs::read(blob_path(&self.blobs(), hash))?
                    };
                    if equivalent_package_xml(&name, &previous, &data) {
                        continue;
                    }
                }
                changes.push(format!("Modified: {name}"));
            } else {
                changes.push(format!("Added: {name}"));
            }
            if changes.len() == limit {
                return Ok(changes);
            }
        }
        let remaining = limit - changes.len();
        changes.extend(entries.keys().take(remaining).map(|name| format!("Removed: {name}")));
        Ok(changes)
    }

    fn diff_working_copy(
        &self,
        file_path: &str,
        use_staged_base: bool,
    ) -> Result<DiffResult> {
        let path = self.resolve_source_path(file_path);
        let key = self.source_key(&path)?;
        let kind = detect_office_kind(&path)?;
        let metadata = self.read_metadata()?;
        let latest = metadata.files.get(&key).and_then(|records| records.last());
        let staged = metadata.staged.get(&key);
        if latest.is_none() && staged.is_none() && !metadata.pending.contains_key(&key) {
            return Err("File is not tracked in this workspace.".into());
        }
        let staged_base = staged.filter(|entry| use_staged_base && entry.snapshot)
            .map(|entry| self.snapshot_record(entry, "Staged".into(), "Staged changes"))
            .transpose()?;
        let baseline = staged_base.as_ref().or(latest);
        let (from, old_text, old_images) = self.diff_base(baseline)?;

        let bytes = fs::read(&path)?;
        let to = DiffVersionInfo {
            id: "Working copy".into(),
            message: "Uncommitted changes".into(),
            created_at: String::new(),
            hash: sha256_bytes(&bytes),
        };
        let mut zip = ZipArchive::new(Cursor::new(bytes))?;
        let new_text = extract_package_text(kind, &mut zip)?;
        let mut new_images = BTreeMap::new();
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index)?;
            if !entry.is_dir() && is_office_image_entry(entry.name()) {
                let mut data = Vec::new();
                entry.read_to_end(&mut data)?;
                new_images.insert(
                    entry.name().to_string(),
                    (sha256_bytes(&data), Some(entry.size())),
                );
            }
        }
        let images = self.diff_images(&old_images, &new_images, true, |name, hash| {
            let mut data = Vec::new();
            zip.by_name(name).ok()?.read_to_end(&mut data).ok()?;
            let mime = image_mime(name)?;
            (sha256_bytes(&data) == hash).then(|| format!("data:{mime};base64,{}", BASE64.encode(data)))
        });

        let mut result = Self::build_diff(from, to, &old_text, &new_text, images);
        if result.added_lines == 0 && result.removed_lines == 0 && result.images.is_empty() {
            if let Some(base) = baseline.filter(|base| base.hash != result.to.hash) {
                let manifest = self.read_manifest(base)?;
                result.package_changes = self.working_package_changes(&manifest, &mut zip, 12)?;
            }
        }
        Ok(result)
    }

    pub fn diff_latest(&self, file_path: &str) -> Result<DiffResult> {
        let records = self.log(Some(file_path))?;
        if records.len() < 2 {
            return Err("At least two versions are required to diff this file.".into());
        }

        let from = &records[records.len() - 2];
        let to = &records[records.len() - 1];
        self.diff_records(Some(from), to, true)
    }

    pub fn diff_versions(&self, file_path: &str, from_id: &str, to_id: &str) -> Result<DiffResult> {
        let records = self.log(Some(file_path))?;
        let from = records
            .iter()
            .find(|item| item.id == from_id)
            .ok_or_else(|| format!("Version not found: {from_id}"))?;
        let to = records
            .iter()
            .find(|item| item.id == to_id)
            .ok_or_else(|| format!("Version not found: {to_id}"))?;
        self.diff_records(Some(from), to, true)
    }

    pub fn checkout(&self, file_path: &str, version: &str, output: &str) -> Result<PathBuf> {
        let records = self.log(Some(file_path))?;
        let record = records
            .iter()
            .find(|item| item.id == version)
            .ok_or_else(|| format!("Version not found: {version}"))?;
        let output = PathBuf::from(output);
        restore_office_package(
            &self.resolve_stored_path(&record.manifest_path),
            &output,
            &self.blobs(),
        )?;
        Ok(output)
    }

    fn read_working_file(&self, file_path: &str) -> Result<(StagedRecord, Vec<u8>)> {
        let path = normalize_existing_path(&self.resolve_source_path(file_path))?;
        let kind = detect_office_kind(&path)?.to_string();
        validate_office_file(&path)?;
        let bytes = fs::read(&path)?;
        let staged = StagedRecord {
            source_path: self.source_key(&path)?,
            original_name: file_name(&path)?,
            kind,
            hash: sha256_bytes(&bytes),
            staged_at: now()?,
            size: bytes.len() as u64,
            snapshot: false,
        };
        Ok((staged, bytes))
    }

    fn snapshot_paths(&self, hash: &str) -> (PathBuf, PathBuf) {
        (
            self.packages().join(format!("{hash}.json")),
            self.snapshots().join(format!("{hash}.txt")),
        )
    }

    fn store_snapshot(&self, staged: &StagedRecord, bytes: Vec<u8>) -> Result<()> {
        self.init()?;
        let (manifest_path, text_path) = self.snapshot_paths(&staged.hash);
        let mut zip = ZipArchive::new(Cursor::new(bytes))?;
        let text = extract_package_text(&staged.kind, &mut zip)?;
        save_office_package(&mut zip, staged, &self.blobs(), &manifest_path)?;
        if !text_path.exists() {
            fs::write(text_path, text)?;
        }
        Ok(())
    }

    fn record_version(
        &self,
        metadata: &mut RepositoryMetadata,
        staged: &StagedRecord,
        message: &str,
    ) -> Result<Option<VersionRecord>> {
        let existing = metadata.files.entry(staged.source_path.clone()).or_default();
        if existing.last().is_some_and(|record| record.hash == staged.hash) {
            return Ok(None);
        }
        let record = self.snapshot_record(staged, format!("v{}", existing.len() + 1), message)?;
        existing.push(record.clone());
        metadata.pending.remove(&staged.source_path);
        Ok(Some(record))
    }

    fn snapshot_record(&self, staged: &StagedRecord, id: String, message: &str) -> Result<VersionRecord> {
        let (manifest_path, text_path) = self.snapshot_paths(&staged.hash);
        Ok(VersionRecord {
            id,
            source_path: staged.source_path.clone(),
            original_name: staged.original_name.clone(),
            kind: staged.kind.clone(),
            hash: staged.hash.clone(),
            object_path: self.stored_key(&manifest_path),
            storage_mode: "package".into(),
            manifest_path: self.stored_key(&manifest_path),
            text_path: self.stored_key(&text_path),
            message: message.into(),
            created_at: now()?,
            size: staged.size,
        })
    }

    fn save_version(&self, file_path: &str, message: &str) -> Result<VersionRecord> {
        let (staged, bytes) = self.read_working_file(file_path)?;
        let mut metadata = self.read_metadata()?;
        if let Some(previous) = metadata.files.get(&staged.source_path)
            .and_then(|records| records.last())
            .filter(|record| record.hash == staged.hash)
        {
            return Ok(previous.clone());
        }
        self.store_snapshot(&staged, bytes)?;
        let record = self.record_version(&mut metadata, &staged, message)?
            .ok_or("The version has already been saved.")?;
        self.write_metadata(&metadata)?;
        Ok(record)
    }

    fn read_metadata(&self) -> Result<RepositoryMetadata> {
        self.init()?;
        let content = fs::read_to_string(self.metadata_path())?;
        let metadata = serde_json::from_str(&content).unwrap_or_default();
        self.normalize_metadata(metadata)
    }

    fn write_metadata(&self, metadata: &RepositoryMetadata) -> Result<()> {
        fs::write(
            self.metadata_path(),
            serde_json::to_string_pretty(metadata)?,
        )?;
        Ok(())
    }

    fn metadata_path(&self) -> PathBuf {
        self.root.join("metadata.json")
    }

    fn objects(&self) -> PathBuf {
        self.root.join("objects")
    }

    fn blobs(&self) -> PathBuf {
        self.root.join("blobs")
    }

    fn packages(&self) -> PathBuf {
        self.root.join("packages")
    }

    fn snapshots(&self) -> PathBuf {
        self.root.join("snapshots")
    }

    fn workspace(&self) -> PathBuf {
        self.root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn source_key(&self, absolute_path: &Path) -> Result<String> {
        let workspace = normalize_existing_path(&self.workspace())?;
        let path = normalize_existing_path(absolute_path)?;
        let relative = path.strip_prefix(&workspace).map_err(|_| {
            format!(
                "Tracked files must be inside the workspace: {}",
                workspace.display()
            )
        })?;
        Ok(path_string(relative))
    }

    fn source_key_for_input(&self, file_path: &str) -> Result<String> {
        let path = Path::new(file_path);
        if path.is_absolute() {
            return self.source_key(&normalize_existing_path(path)?);
        }
        Ok(path_string(path))
    }

    fn resolve_source_path(&self, file_path: &str) -> PathBuf {
        let path = PathBuf::from(file_path);
        if path.is_absolute() {
            path
        } else {
            self.workspace().join(path)
        }
    }

    fn stored_key(&self, path: &Path) -> String {
        path.strip_prefix(&self.workspace())
            .map(path_string)
            .unwrap_or_else(|_| path_string(path))
    }

    fn resolve_stored_path(&self, path: &str) -> PathBuf {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            path
        } else {
            self.workspace().join(path)
        }
    }

    fn read_manifest(&self, record: &VersionRecord) -> Result<PackageManifest> {
        let path = self.resolve_stored_path(&record.manifest_path);
        Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
    }

    fn version_text(&self, record: &VersionRecord, manifest: &PackageManifest) -> Result<String> {
        if !matches!(record.kind.as_str(), "docx" | "pptx") {
            return Ok(fs::read_to_string(self.resolve_stored_path(&record.text_path))
                .unwrap_or_default());
        }

        // Earlier snapshots may include Word instructions or PowerPoint field
        // caches. Use the same extraction rules for both sides.
        let parts = manifest.entries.iter().filter(|entry| !entry.dir)
            .map(|entry| (entry.name.as_str(), entry))
            .collect::<BTreeMap<_, _>>();
        let read_part = |name: &str| {
            let entry = parts[name];
            let hash = entry.hash.as_deref().ok_or("Missing document part hash")?;
            let data = if entry.compressed {
                read_blob_data(&self.blobs(), hash)?
            } else {
                fs::read(blob_path(&self.blobs(), hash))?
            };
            Ok(String::from_utf8(data)?)
        };
        match record.kind.as_str() {
            "docx" => extract_docx_parts(parts.keys().copied(), read_part),
            _ => extract_pptx_parts(parts.keys().copied(), read_part),
        }
    }

    fn diff_base(&self, record: Option<&VersionRecord>) -> Result<(DiffVersionInfo, String, ImageEntries)> {
        let Some(record) = record else {
            return Ok((
                DiffVersionInfo {
                    id: "Empty".into(),
                    message: "No committed version".into(),
                    created_at: String::new(),
                    hash: String::new(),
                },
                String::new(),
                BTreeMap::new(),
            ));
        };
        let manifest = self.read_manifest(record)?;
        Ok((
            record.into(),
            self.version_text(record, &manifest)?,
            Self::image_entries(&manifest),
        ))
    }

    fn diff_records(&self, from: Option<&VersionRecord>, to: &VersionRecord, previews: bool) -> Result<DiffResult> {
        let (from, old_text, old_images) = self.diff_base(from)?;
        let new_manifest = self.read_manifest(to)?;
        let images = self.diff_images(
            &old_images,
            &Self::image_entries(&new_manifest),
            previews,
            |name, hash| self.image_data_url(name, hash).ok(),
        );
        Ok(Self::build_diff(
            from,
            to.into(),
            &old_text,
            &self.version_text(to, &new_manifest)?,
            images,
        ))
    }

    fn build_diff(
        from: DiffVersionInfo,
        to: DiffVersionInfo,
        old_text: &str,
        new_text: &str,
        all_images: Vec<DiffImage>,
    ) -> DiffResult {
        let diff = TextDiff::from_lines(old_text, new_text);
        let mut added_lines = 0;
        let mut removed_lines = 0;
        let mut unchanged_lines = 0;
        let mut lines = Vec::new();

        for change in diff.iter_all_changes() {
            let kind = match change.tag() {
                ChangeTag::Equal => {
                    unchanged_lines += 1;
                    "context"
                }
                ChangeTag::Delete => {
                    removed_lines += 1;
                    "removed"
                }
                ChangeTag::Insert => {
                    added_lines += 1;
                    "added"
                }
            };
            lines.push(DiffLine {
                kind: kind.to_string(),
                old_line: change.old_index().map(|index| index + 1),
                new_line: change.new_index().map(|index| index + 1),
                text: change
                    .value()
                    .trim_end_matches(&['\r', '\n'][..])
                    .to_string(),
            });
        }

        let added_images = all_images
            .iter()
            .filter(|item| item.kind == "added")
            .count();
        let removed_images = all_images
            .iter()
            .filter(|item| item.kind == "removed")
            .count();
        let modified_images = all_images
            .iter()
            .filter(|item| item.kind == "modified")
            .count();
        let unchanged_images = all_images
            .iter()
            .filter(|item| item.kind == "unchanged")
            .count();
        let images = all_images
            .into_iter()
            .filter(|item| item.kind != "unchanged")
            .collect();

        DiffResult {
            from,
            to,
            added_lines,
            removed_lines,
            unchanged_lines,
            lines,
            added_images,
            removed_images,
            modified_images,
            unchanged_images,
            images,
            package_changes: Vec::new(),
        }
    }

    fn diff_images(
        &self,
        old_images: &ImageEntries,
        new_images: &ImageEntries,
        previews: bool,
        mut new_preview: impl FnMut(&str, &str) -> Option<String>,
    ) -> Vec<DiffImage> {
        let mut keys = old_images.keys().cloned().collect::<BTreeSet<_>>();
        keys.extend(new_images.keys().cloned());

        let mut images = Vec::new();
        for path in keys {
            let old_entry = old_images.get(&path);
            let new_entry = new_images.get(&path);
            let old_hash = old_entry.map(|(hash, _)| hash.clone());
            let new_hash = new_entry.map(|(hash, _)| hash.clone());
            let kind = match (&old_hash, &new_hash) {
                (Some(old), Some(new)) if old == new => "unchanged",
                (Some(_), Some(_)) => "modified",
                (Some(_), None) => "removed",
                (None, Some(_)) => "added",
                _ => continue,
            };

            let preview_supported = image_mime(&path).is_some();
            let include_preview = previews && preview_supported && kind != "unchanged";
            images.push(DiffImage {
                kind: kind.to_string(),
                path: path.clone(),
                old_data_url: old_hash
                    .as_deref()
                    .filter(|_| include_preview)
                    .and_then(|hash| self.image_data_url(&path, hash).ok()),
                new_data_url: new_hash
                    .as_deref()
                    .filter(|_| include_preview)
                    .and_then(|hash| new_preview(&path, hash)),
                old_hash,
                new_hash,
                old_size: old_entry.and_then(|(_, size)| *size),
                new_size: new_entry.and_then(|(_, size)| *size),
                preview_supported,
            });
        }

        images
    }

    fn image_entries(manifest: &PackageManifest) -> ImageEntries {
        manifest
            .entries
            .iter()
            .filter(|entry| !entry.dir && is_office_image_entry(&entry.name))
            .filter_map(|entry| {
                entry.hash.as_ref().map(|hash| (entry.name.clone(), (hash.clone(), entry.size)))
            })
            .collect()
    }

    fn image_data_url(&self, name: &str, hash: &str) -> Result<String> {
        let Some(mime) = image_mime(name) else {
            return Err(format!("Unsupported image preview format: {name}").into());
        };
        let data = read_blob_data(&self.blobs(), hash)?;
        Ok(format!("data:{mime};base64,{}", BASE64.encode(data)))
    }

    fn normalize_metadata(&self, metadata: RepositoryMetadata) -> Result<RepositoryMetadata> {
        let mut files = HashMap::new();
        for (key, records) in metadata.files {
            let normalized_key = self.normalize_source_key(&key);
            let normalized_records = records
                .into_iter()
                .map(|mut record| {
                    record.source_path = self.normalize_source_key(&record.source_path);
                    record.object_path = self.normalize_stored_key(&record.object_path);
                    record.manifest_path = self.normalize_stored_key(&record.manifest_path);
                    record.text_path = self.normalize_stored_key(&record.text_path);
                    record
                })
                .collect::<Vec<_>>();
            files
                .entry(normalized_key)
                .or_insert_with(Vec::new)
                .extend(normalized_records);
        }

        let normalize_entries = |entries: HashMap<String, StagedRecord>| {
            entries.into_iter().map(|(key, mut record)| {
                let normalized_key = self.normalize_source_key(&key);
                record.source_path = self.normalize_source_key(&record.source_path);
                (normalized_key, record)
            }).collect()
        };
        Ok(RepositoryMetadata {
            schema_version: metadata.schema_version,
            files,
            staged: normalize_entries(metadata.staged),
            pending: normalize_entries(metadata.pending),
        })
    }

    fn normalize_source_key(&self, value: &str) -> String {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            self.source_key(&path).unwrap_or_else(|_| value.to_string())
        } else {
            value.to_string()
        }
    }

    fn normalize_stored_key(&self, value: &str) -> String {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            self.stored_key(&path)
        } else {
            value.to_string()
        }
    }
}

fn save_office_package<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    staged: &StagedRecord,
    blobs: &Path,
    manifest_path: &Path,
) -> Result<()> {
    if manifest_path.exists() {
        return Ok(());
    }

    let mut entries = Vec::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = entry.name().to_string();
        if entry.is_dir() {
            entries.push(PackageManifestEntry {
                name,
                dir: true,
                hash: None,
                size: None,
                compressed: false,
            });
            continue;
        }

        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        let hash = sha256_bytes(&data);
        let blob_path = blob_path(blobs, &hash);
        if !blob_path.exists() {
            if let Some(parent) = blob_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut gz = GzEncoder::new(Vec::new(), Compression::default());
            gz.write_all(&data)?;
            fs::write(&blob_path, gz.finish()?)?;
        }
        entries.push(PackageManifestEntry {
            name,
            dir: false,
            hash: Some(hash),
            size: Some(data.len() as u64),
            compressed: true,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    let manifest = PackageManifest {
        schema_version: 1,
        source_hash: staged.hash.clone(),
        original_name: staged.original_name.clone(),
        created_at: now()?,
        entries,
    };
    fs::write(manifest_path, serde_json::to_string_pretty(&manifest)?)?;
    Ok(())
}

fn restore_office_package(manifest_path: &Path, target_path: &Path, blobs: &Path) -> Result<()> {
    let manifest: PackageManifest = serde_json::from_str(&fs::read_to_string(manifest_path)?)?;
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::create(target_path)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for entry in manifest.entries {
        if entry.dir {
            zip.add_directory(entry.name, options)?;
            continue;
        }
        let hash = entry
            .hash
            .ok_or_else(|| format!("Missing blob hash for {}", entry.name))?;
        let raw = fs::read(blob_path(blobs, &hash))?;
        let data = if entry.compressed {
            let mut decoder = GzDecoder::new(&raw[..]);
            let mut out = Vec::new();
            decoder.read_to_end(&mut out)?;
            out
        } else {
            raw
        };
        zip.start_file(entry.name, options)?;
        zip.write_all(&data)?;
    }
    zip.finish()?;
    Ok(())
}

fn read_blob_data(blobs: &Path, hash: &str) -> Result<Vec<u8>> {
    let raw = fs::read(blob_path(blobs, hash))?;
    let mut decoder = GzDecoder::new(&raw[..]);
    let mut data = Vec::new();
    decoder.read_to_end(&mut data)?;
    Ok(data)
}

fn is_office_image_entry(name: &str) -> bool {
    let normalized = name.replace('\\', "/").to_ascii_lowercase();
    (normalized.starts_with("word/media/")
        || normalized.starts_with("ppt/media/")
        || normalized.starts_with("xl/media/"))
        && matches!(
            Path::new(&normalized)
                .extension()
                .and_then(|value| value.to_str()),
            Some(
                "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "bmp"
                    | "webp"
                    | "svg"
                    | "tif"
                    | "tiff"
                    | "emf"
                    | "wmf"
            )
        )
}

fn image_mime(name: &str) -> Option<&'static str> {
    match Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("bmp") => Some("image/bmp"),
        Some("webp") => Some("image/webp"),
        Some("svg") => Some("image/svg+xml"),
        _ => None,
    }
}

fn validate_office_file(path: &Path) -> Result<fs::Metadata> {
    if !is_supported_office_file(path) {
        return Err("Only .docx, .xlsx, and .pptx files are supported.".into());
    }
    let stat = fs::metadata(path)?;
    if !stat.is_file() {
        return Err(format!("Not a file: {}", path.display()).into());
    }
    if stat.len() < 4 {
        return Err(format!("Invalid Office file: {} is too small.", path.display()).into());
    }
    let mut signature = [0_u8; 2];
    File::open(path)?.read_exact(&mut signature)?;
    if signature != [0x50, 0x4b] {
        return Err(format!(
            "Invalid Office file: {} is not a zipped Office document.",
            path.display()
        )
        .into());
    }
    Ok(stat)
}

fn is_save_metadata(name: &str) -> bool {
    matches!(
        name,
        "docProps/core.xml"
            | "docProps/app.xml"
            | "docProps/thumbnail.jpeg"
            | "ppt/viewProps.xml"
            | "ppt/_rels/viewProps.xml.rels"
    )
}

fn working_hash(path: &Path) -> Result<Option<String>> {
    match sha256_file(path) {
        Ok(hash) => Ok(Some(hash)),
        Err(error) if error.downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let bytes = file.read(&mut buffer)?;
        if bytes == 0 {
            break;
        }
        hasher.update(&buffer[..bytes]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn sha256_bytes(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn blob_path(root: &Path, hash: &str) -> PathBuf {
    root.join(&hash[..2]).join(hash)
}

fn normalize_existing_path(path: &Path) -> Result<PathBuf> {
    path.canonicalize().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "File not found or inaccessible: {} ({error})",
                path.display()
            ),
        )
        .into()
    })
}

fn file_name(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .ok_or_else(|| format!("Invalid file name: {}", path.display()).into())
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn now() -> Result<String> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

#[cfg(windows)]
fn try_set_hidden(path: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::process::Command::new("attrib")
        .arg("+H")
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

#[cfg(not(windows))]
fn try_set_hidden(_path: &Path) {}
