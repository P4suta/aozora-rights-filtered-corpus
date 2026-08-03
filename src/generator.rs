use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Datelike, NaiveDate};
use encoding_rs::SHIFT_JIS;
use rayon::prelude::*;
use reqwest::blocking::Client;
use serde::Serialize;
use sha2::{Digest, Sha256};
use url::Url;
use zip::ZipArchive;

use crate::model::{
    Config, Contributor, CorpusEntry, CorpusInfo, CorpusManifest, CsvRow, EditionDraft,
    FirstPublication, QuarantineEntry, QuarantineManifest, QuarantineSummary, RejectionReason,
};
use crate::publication;

const CORPUS_REPOSITORY: &str = "https://github.com/P4suta/aozora-rights-filtered-corpus";
const UPSTREAM_REPOSITORY: &str = "https://github.com/aozorabunko/aozorabunko";
const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub enum Input {
    Remote,
    Checkout(PathBuf),
}

#[derive(Debug)]
pub struct ScanReport {
    pub csv_rows: usize,
    pub edition_groups: usize,
    pub candidates: Vec<EditionDraft>,
    pub quarantine: Vec<QuarantineEntry>,
    pub csv_sha256: String,
}

#[derive(Debug)]
struct ProcessedEdition {
    entry: CorpusEntry,
    source: Vec<u8>,
}

#[derive(Debug)]
struct ProcessFailure {
    reason: RejectionReason,
    detail: String,
}

#[derive(Clone)]
struct UpstreamReader {
    config: Arc<Config>,
    input: Input,
    client: Client,
}

impl UpstreamReader {
    fn new(config: Arc<Config>, input: Input) -> Result<Self> {
        if let Input::Checkout(root) = &input {
            verify_checkout(root, &config.upstream.commit)?;
        }
        let client = Client::builder()
            .user_agent("aozora-rights-filtered-corpus/0.1")
            .build()
            .context("build HTTP client")?;
        Ok(Self {
            config,
            input,
            client,
        })
    }

    fn read(&self, relative: &str, maximum: u64) -> Result<Vec<u8>> {
        let safe = safe_relative_path(relative)?;
        match &self.input {
            Input::Checkout(root) => {
                let path = root.join(&safe);
                let metadata = fs::metadata(&path)
                    .with_context(|| format!("read upstream metadata {}", path.display()))?;
                if metadata.len() > maximum {
                    bail!("upstream file exceeds {maximum} bytes: {}", path.display());
                }
                fs::read(&path).with_context(|| format!("read upstream file {}", path.display()))
            }
            Input::Remote => {
                let url = format!(
                    "https://raw.githubusercontent.com/aozorabunko/aozorabunko/{}/{}",
                    self.config.upstream.commit,
                    safe.to_string_lossy()
                );
                let response = self
                    .client
                    .get(&url)
                    .send()
                    .with_context(|| format!("download fixed upstream file {url}"))?
                    .error_for_status()
                    .with_context(|| format!("fixed upstream file is unavailable {url}"))?;
                if response
                    .content_length()
                    .is_some_and(|length| length > maximum)
                {
                    bail!("upstream response exceeds {maximum} bytes: {url}");
                }
                let mut bytes = Vec::new();
                response
                    .take(maximum + 1)
                    .read_to_end(&mut bytes)
                    .with_context(|| format!("read fixed upstream response {url}"))?;
                if bytes.len() as u64 > maximum {
                    bail!("upstream response exceeds {maximum} bytes: {url}");
                }
                Ok(bytes)
            }
        }
    }
}

pub fn load_config(path: &Path) -> Result<Config> {
    let bytes = fs::read(path).with_context(|| format!("read config {}", path.display()))?;
    let config: Config =
        toml::from_slice(&bytes).with_context(|| format!("parse config {}", path.display()))?;
    validate_config(&config)?;
    Ok(config)
}

pub fn validate_config(config: &Config) -> Result<()> {
    let date = NaiveDate::parse_from_str(&config.policy.reference_date, "%Y-%m-%d")
        .context("policy.reference_date must be an ISO date")?;
    let expected_cutoff = date
        .year()
        .checked_sub(96)
        .and_then(|year| u16::try_from(year).ok())
        .context("derive cutoff year")?;
    if config.policy.cutoff_year != expected_cutoff {
        bail!(
            "policy.cutoff_year must be {} for {}",
            expected_cutoff,
            config.policy.reference_date
        );
    }
    if config.upstream.repository != UPSTREAM_REPOSITORY {
        bail!("upstream.repository must be {UPSTREAM_REPOSITORY}");
    }
    validate_commit(&config.upstream.commit)?;
    validate_sha256(&config.upstream.metadata_sha256)?;
    validate_official_url(&config.upstream.metadata_url, ".zip")?;
    safe_relative_path(&config.upstream.metadata_path)?;
    for output in [
        &config.output.manifest,
        &config.output.quarantine,
        &config.output.sources,
    ] {
        safe_relative_path(output)?;
    }
    Ok(())
}

pub fn scan(config: &Config, input: Input) -> Result<ScanReport> {
    let reader = UpstreamReader::new(Arc::new(config.clone()), input)?;
    let metadata_zip = reader
        .read(&config.upstream.metadata_path, MAX_ARCHIVE_BYTES)
        .context("read metadata ZIP")?;
    let metadata_sha256 = sha256(&metadata_zip);
    if metadata_sha256 != config.upstream.metadata_sha256 {
        bail!(
            "metadata ZIP SHA-256 mismatch: expected {}, got {}",
            config.upstream.metadata_sha256,
            metadata_sha256
        );
    }
    let csv_bytes = extract_metadata_csv(&metadata_zip)?;
    scan_csv(config, &csv_bytes)
}

pub fn scan_csv(config: &Config, csv_bytes: &[u8]) -> Result<ScanReport> {
    let bytes = csv_bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(csv_bytes);
    let csv_sha256 = sha256(bytes);
    let mut csv = csv::ReaderBuilder::new().flexible(false).from_reader(bytes);
    let rows = csv
        .deserialize::<CsvRow>()
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("parse extended metadata CSV")?;
    let csv_rows = rows.len();
    let mut groups: BTreeMap<(String, String), Vec<CsvRow>> = BTreeMap::new();
    for row in rows {
        groups
            .entry((row.work_id.clone(), row.archive_url.clone()))
            .or_default()
            .push(row);
    }
    let edition_groups = groups.len();
    let mut candidates = Vec::new();
    let mut quarantine = Vec::new();
    let mut edition_ids = BTreeSet::new();
    for ((work_id, archive_url), rows) in groups {
        match draft(config, &work_id, &archive_url, &rows) {
            Ok(candidate) if edition_ids.insert(candidate.edition_id.clone()) => {
                candidates.push(candidate);
            }
            Ok(candidate) => quarantine.push(QuarantineEntry {
                edition_id: Some(candidate.edition_id),
                work_id,
                archive_url,
                reasons: vec![RejectionReason::DuplicateEdition],
                detail: "duplicate edition identity after normalization".into(),
            }),
            Err(mut rejected) => {
                rejected.reasons.sort();
                rejected.reasons.dedup();
                quarantine.push(rejected);
            }
        }
    }
    candidates.sort_by(|left, right| left.edition_id.cmp(&right.edition_id));
    quarantine.sort_by(|left, right| {
        (&left.work_id, &left.archive_url).cmp(&(&right.work_id, &right.archive_url))
    });
    Ok(ScanReport {
        csv_rows,
        edition_groups,
        candidates,
        quarantine,
        csv_sha256,
    })
}

fn draft(
    config: &Config,
    work_id: &str,
    archive_url: &str,
    rows: &[CsvRow],
) -> std::result::Result<EditionDraft, QuarantineEntry> {
    let edition_id = valid_work_id(work_id).then(|| make_edition_id(work_id, archive_url));
    let mut reasons = Vec::new();
    let mut detail = Vec::new();
    if edition_id.is_none() {
        reasons.push(RejectionReason::InvalidWorkId);
        detail.push("work ID is not six ASCII digits");
    }
    let title = uniform(rows, |row| &row.title, "title", &mut reasons, &mut detail)
        .trim()
        .to_owned();
    let reading = uniform(
        rows,
        |row| &row.reading,
        "reading",
        &mut reasons,
        &mut detail,
    )
    .trim()
    .to_owned();
    if title.is_empty() || reading.is_empty() {
        reasons.push(RejectionReason::MetadataMismatch);
        detail.push("title or reading is empty after trimming metadata padding");
    }
    let first_raw = uniform(
        rows,
        |row| &row.first_publication,
        "first publication",
        &mut reasons,
        &mut detail,
    );
    let card_url = uniform(
        rows,
        |row| &row.card_url,
        "card URL",
        &mut reasons,
        &mut detail,
    );
    let encoding = uniform(
        rows,
        |row| &row.encoding,
        "text encoding",
        &mut reasons,
        &mut detail,
    );
    if rows.iter().any(|row| row.work_copyright != "なし") {
        reasons.push(RejectionReason::WorkCopyright);
        detail.push("at least one work copyright flag is not なし");
    }
    if validate_official_url(archive_url, ".zip").is_err() {
        reasons.push(RejectionReason::InvalidArchiveUrl);
        detail.push("text ZIP URL is not an official Aozora HTTPS URL");
    }
    if validate_official_url(&card_url, ".html").is_err() {
        reasons.push(RejectionReason::InvalidCardUrl);
        detail.push("card URL is not an official Aozora HTTPS URL");
    }
    if !matches!(
        encoding.as_str(),
        "ShiftJIS" | "Shift_JIS" | "UTF-8" | "UTF8"
    ) {
        reasons.push(RejectionReason::UnsupportedEncoding);
        detail.push("text encoding is not ShiftJIS or UTF-8");
    }

    let mut contributors = BTreeSet::new();
    for row in rows {
        let name = format!("{} {}", row.surname.trim(), row.given_name.trim())
            .trim()
            .to_owned();
        if row.contributor_id.trim().is_empty() || row.role.trim().is_empty() || name.is_empty() {
            reasons.push(RejectionReason::EmptyContributor);
            detail.push("a contributor ID, role, or name is empty");
        }
        if row.contributor_copyright != "なし" {
            reasons.push(RejectionReason::ContributorCopyright);
            detail.push("at least one contributor copyright flag is not なし");
        }
        contributors.insert(Contributor {
            role: row.role.trim().to_owned(),
            name,
            copyright: row.contributor_copyright.clone(),
        });
    }
    if contributors.is_empty() {
        reasons.push(RejectionReason::EmptyContributor);
        detail.push("edition has no contributors");
    }

    let publication_years = match publication::parse(&first_raw, config.policy.cutoff_year) {
        Ok(years) => years,
        Err(reason) => {
            reasons.push(reason);
            detail.push("first publication did not satisfy the conservative year parser");
            Vec::new()
        }
    };
    if !reasons.is_empty() {
        return Err(QuarantineEntry {
            edition_id,
            work_id: work_id.to_owned(),
            archive_url: archive_url.to_owned(),
            reasons,
            detail: detail.join("; "),
        });
    }
    Ok(EditionDraft {
        edition_id: edition_id.expect("validated above"),
        work_id: work_id.to_owned(),
        title,
        reading,
        contributors: contributors.into_iter().collect(),
        first_publication: FirstPublication {
            raw: first_raw,
            years: publication_years,
        },
        card_url,
        archive_url: archive_url.to_owned(),
        encoding,
    })
}

fn uniform(
    rows: &[CsvRow],
    get: impl Fn(&CsvRow) -> &String,
    label: &'static str,
    reasons: &mut Vec<RejectionReason>,
    detail: &mut Vec<&'static str>,
) -> String {
    let values = rows
        .iter()
        .map(|row| get(row).as_str())
        .collect::<BTreeSet<_>>();
    if values.len() != 1 {
        reasons.push(RejectionReason::MetadataMismatch);
        detail.push(label);
    }
    values.first().copied().unwrap_or_default().to_owned()
}

pub fn snapshot(config_path: &Path, input: Input, jobs: usize) -> Result<()> {
    if jobs == 0 {
        bail!("jobs must be greater than zero");
    }
    let config = Arc::new(load_config(config_path)?);
    let root = config_path.parent().unwrap_or_else(|| Path::new("."));
    let report = scan(&config, input.clone())?;
    if report.candidates.len() + config.policy.max_accepted_decrease
        < config.policy.expected_accepted_editions
    {
        bail!(
            "candidate count dropped below the guarded floor: previous accepted {}, candidates {}; inspect quarantine before changing policy",
            config.policy.expected_accepted_editions,
            report.candidates.len()
        );
    }
    let reader = UpstreamReader::new(Arc::clone(&config), input)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .build()
        .context("build download worker pool")?;
    let processed = pool.install(|| {
        report
            .candidates
            .par_iter()
            .map(|draft| process_edition(&config, &reader, draft, &report.csv_sha256))
            .collect::<Vec<_>>()
    });

    let mut entries = Vec::new();
    let mut sources = Vec::new();
    let mut quarantine = report.quarantine;
    for (draft, result) in report.candidates.iter().zip(processed) {
        match result {
            Ok(processed) => {
                sources.push((processed.entry.utf8_filename.clone(), processed.source));
                entries.push(processed.entry);
            }
            Err(failure) => quarantine.push(QuarantineEntry {
                edition_id: Some(draft.edition_id.clone()),
                work_id: draft.work_id.clone(),
                archive_url: draft.archive_url.clone(),
                reasons: vec![failure.reason],
                detail: failure.detail,
            }),
        }
    }
    entries.sort_by(|left, right| left.edition_id.cmp(&right.edition_id));
    quarantine.sort_by(|left, right| {
        (&left.work_id, &left.archive_url).cmp(&(&right.work_id, &right.archive_url))
    });
    if entries.len() != config.policy.expected_accepted_editions {
        bail!(
            "accepted edition count drift: expected {}, got {}; {} candidate archives failed",
            config.policy.expected_accepted_editions,
            entries.len(),
            report.candidates.len() - entries.len()
        );
    }

    write_sources(root, &config.output.sources, &sources)?;
    let manifest = CorpusManifest {
        schema_version: 1,
        corpus: CorpusInfo {
            repository: CORPUS_REPOSITORY.into(),
            commit: config.upstream.commit.clone(),
            reference_date: config.policy.reference_date.clone(),
            cutoff_year: config.policy.cutoff_year,
            metadata_url: config.upstream.metadata_url.clone(),
            metadata_sha256: config.upstream.metadata_sha256.clone(),
        },
        entries,
    };
    let quarantine_manifest = QuarantineManifest {
        schema_version: 1,
        reference_date: config.policy.reference_date.clone(),
        cutoff_year: config.policy.cutoff_year,
        upstream_commit: config.upstream.commit.clone(),
        metadata_sha256: config.upstream.metadata_sha256.clone(),
        summary: QuarantineSummary {
            csv_rows: report.csv_rows,
            edition_groups: report.edition_groups,
            candidate_editions: report.candidates.len(),
            accepted_editions: manifest.entries.len(),
            quarantined_editions: quarantine.len(),
        },
        entries: quarantine,
    };
    write_json(root.join(&config.output.manifest), &manifest)?;
    write_json(root.join(&config.output.quarantine), &quarantine_manifest)?;
    verify(config_path)
}

fn process_edition(
    config: &Config,
    reader: &UpstreamReader,
    draft: &EditionDraft,
    csv_sha256: &str,
) -> std::result::Result<ProcessedEdition, ProcessFailure> {
    let relative = archive_relative_path(&draft.archive_url).map_err(|error| ProcessFailure {
        reason: RejectionReason::InvalidArchiveUrl,
        detail: error.to_string(),
    })?;
    let archive = reader
        .read(&relative, MAX_ARCHIVE_BYTES)
        .map_err(|error| ProcessFailure {
            reason: RejectionReason::ArchiveDownloadFailed,
            detail: error.to_string(),
        })?;
    let archive_sha256 = sha256(&archive);
    let (archive_filename, source_bytes) =
        extract_text(&archive).map_err(|error| ProcessFailure {
            reason: RejectionReason::ArchiveWithoutSingleText,
            detail: error.to_string(),
        })?;
    let source = decode_source(&source_bytes, &draft.encoding).map_err(|error| ProcessFailure {
        reason: RejectionReason::SourceDecodeFailed,
        detail: error.to_string(),
    })?;
    let utf8_filename = format!("{}.txt", draft.edition_id);
    let utf8_sha256 = sha256(&source);
    Ok(ProcessedEdition {
        entry: CorpusEntry {
            edition_id: draft.edition_id.clone(),
            work_id: draft.work_id.clone(),
            title: draft.title.clone(),
            reading: draft.reading.clone(),
            copyright: "なし".into(),
            contributors: draft.contributors.clone(),
            first_publication: draft.first_publication.clone(),
            card_url: draft.card_url.clone(),
            archive_url: draft.archive_url.clone(),
            archive_filename,
            utf8_filename,
            upstream_commit: config.upstream.commit.clone(),
            csv_sha256: csv_sha256.to_owned(),
            archive_sha256,
            utf8_sha256,
        },
        source,
    })
}

fn extract_metadata_csv(archive: &[u8]) -> Result<Vec<u8>> {
    let mut zip = ZipArchive::new(Cursor::new(archive)).context("open metadata ZIP")?;
    let indexes = (0..zip.len())
        .filter(|index| {
            zip.by_index(*index)
                .ok()
                .is_some_and(|file| file.name().ends_with(".csv") && !file.is_dir())
        })
        .collect::<Vec<_>>();
    if indexes.len() != 1 {
        bail!("metadata ZIP must contain exactly one CSV");
    }
    let mut file = zip.by_index(indexes[0]).context("open metadata CSV")?;
    if file.size() > MAX_ARCHIVE_BYTES {
        bail!("metadata CSV is too large");
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).context("read metadata CSV")?;
    Ok(bytes)
}

fn extract_text(archive: &[u8]) -> Result<(String, Vec<u8>)> {
    let mut zip = ZipArchive::new(Cursor::new(archive)).context("open text ZIP")?;
    let indexes = (0..zip.len())
        .filter(|index| {
            zip.by_index(*index).ok().is_some_and(|file| {
                !file.is_dir()
                    && file
                        .enclosed_name()
                        .as_deref()
                        .and_then(Path::extension)
                        .and_then(|value| value.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("txt"))
            })
        })
        .collect::<Vec<_>>();
    if indexes.len() != 1 {
        bail!("text ZIP must contain exactly one safe .txt file");
    }
    let mut file = zip.by_index(indexes[0]).context("open text member")?;
    if file.size() > MAX_SOURCE_BYTES {
        bail!("text member exceeds {MAX_SOURCE_BYTES} bytes");
    }
    let path = file
        .enclosed_name()
        .context("text member has an unsafe path")?
        .to_owned();
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .context("text member filename is not UTF-8")?
        .to_owned();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).context("read text member")?;
    Ok((filename, bytes))
}

fn decode_source(bytes: &[u8], encoding: &str) -> Result<Vec<u8>> {
    let text = match encoding {
        "ShiftJIS" | "Shift_JIS" => {
            let (decoded, had_errors) = SHIFT_JIS.decode_without_bom_handling(bytes);
            if had_errors {
                bail!("Shift_JIS source contains invalid byte sequences");
            }
            decoded.into_owned()
        }
        "UTF-8" | "UTF8" => std::str::from_utf8(bytes)
            .context("source declared UTF-8 is invalid")?
            .strip_prefix('\u{feff}')
            .unwrap_or_else(|| std::str::from_utf8(bytes).expect("validated above"))
            .to_owned(),
        _ => bail!("unsupported source encoding {encoding}"),
    };
    if text.contains('\u{fffd}') {
        bail!("decoded source contains U+FFFD");
    }
    Ok(text.into_bytes())
}

pub fn verify(config_path: &Path) -> Result<()> {
    let config = load_config(config_path)?;
    let root = config_path.parent().unwrap_or_else(|| Path::new("."));
    let manifest_path = root.join(&config.output.manifest);
    let quarantine_path = root.join(&config.output.quarantine);
    let manifest: CorpusManifest = read_json(&manifest_path)?;
    let quarantine: QuarantineManifest = read_json(&quarantine_path)?;
    if manifest.schema_version != 1 || quarantine.schema_version != 1 {
        bail!("generated manifests must use schemaVersion 1");
    }
    if manifest.corpus.repository != CORPUS_REPOSITORY
        || manifest.corpus.commit != config.upstream.commit
        || manifest.corpus.reference_date != config.policy.reference_date
        || manifest.corpus.cutoff_year != config.policy.cutoff_year
        || manifest.corpus.metadata_url != config.upstream.metadata_url
        || manifest.corpus.metadata_sha256 != config.upstream.metadata_sha256
    {
        bail!("manifest provenance differs from corpus.toml");
    }
    if manifest.entries.len() != config.policy.expected_accepted_editions {
        bail!("manifest entry count differs from fixed snapshot");
    }
    let source_root = root.join(&config.output.sources);
    let mut expected_files = BTreeSet::new();
    let mut identities = BTreeSet::new();
    let mut previous = None;
    for entry in &manifest.entries {
        if previous
            .as_ref()
            .is_some_and(|value| value >= &entry.edition_id)
        {
            bail!("manifest entries are not strictly sorted");
        }
        previous = Some(entry.edition_id.clone());
        if !valid_work_id(&entry.work_id)
            || entry.edition_id != make_edition_id(&entry.work_id, &entry.archive_url)
            || entry.copyright != "なし"
            || entry.contributors.is_empty()
            || entry
                .contributors
                .iter()
                .any(|contributor| contributor.copyright != "なし")
            || entry.first_publication.years.is_empty()
            || entry
                .first_publication
                .years
                .iter()
                .any(|year| *year > config.policy.cutoff_year)
            || entry.upstream_commit != config.upstream.commit
        {
            bail!(
                "entry {} violates rights manifest invariants",
                entry.edition_id
            );
        }
        validate_sha256(&entry.csv_sha256)?;
        validate_sha256(&entry.archive_sha256)?;
        validate_sha256(&entry.utf8_sha256)?;
        validate_official_url(&entry.card_url, ".html")?;
        validate_official_url(&entry.archive_url, ".zip")?;
        if !identities.insert((&entry.work_id, &entry.archive_url)) {
            bail!("duplicate work ID + archive URL identity");
        }
        if entry.utf8_filename != format!("{}.txt", entry.edition_id) {
            bail!("UTF-8 filename does not match edition ID");
        }
        expected_files.insert(entry.utf8_filename.clone());
        let source = fs::read(source_root.join(&entry.utf8_filename))
            .with_context(|| format!("read source {}", entry.utf8_filename))?;
        std::str::from_utf8(&source)
            .with_context(|| format!("source is not UTF-8 {}", entry.utf8_filename))?;
        if sha256(&source) != entry.utf8_sha256 {
            bail!("UTF-8 source hash mismatch {}", entry.edition_id);
        }
    }
    let actual_files = fs::read_dir(&source_root)
        .with_context(|| format!("read source directory {}", source_root.display()))?
        .map(|entry| {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                bail!("source directory contains a non-file entry");
            }
            entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow!("source filename is not UTF-8"))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    if actual_files != expected_files {
        bail!("source directory contains missing or untracked files");
    }
    if quarantine.summary.accepted_editions != manifest.entries.len()
        || quarantine.summary.candidate_editions < manifest.entries.len()
        || quarantine.summary.quarantined_editions != quarantine.entries.len()
    {
        bail!("quarantine summary counts do not match manifests");
    }
    Ok(())
}

pub fn diff_summary(config_path: &Path, base_manifest: Option<&Path>) -> Result<String> {
    let config = load_config(config_path)?;
    let root = config_path.parent().unwrap_or_else(|| Path::new("."));
    let current: CorpusManifest = read_json(&root.join(&config.output.manifest))?;
    let Some(base_path) = base_manifest else {
        return Ok(format!(
            "initial snapshot: {} accepted editions",
            current.entries.len()
        ));
    };
    let base: CorpusManifest = read_json(base_path)?;
    let before = base
        .entries
        .iter()
        .map(|entry| (&entry.edition_id, &entry.utf8_sha256))
        .collect::<BTreeMap<_, _>>();
    let after = current
        .entries
        .iter()
        .map(|entry| (&entry.edition_id, &entry.utf8_sha256))
        .collect::<BTreeMap<_, _>>();
    let added = after
        .keys()
        .filter(|key| !before.contains_key(*key))
        .count();
    let removed = before
        .keys()
        .filter(|key| !after.contains_key(*key))
        .count();
    let changed = after
        .iter()
        .filter(|(key, digest)| before.get(*key).is_some_and(|before| before != *digest))
        .count();
    Ok(format!(
        "accepted editions: {} -> {}; added {added}, removed {removed}, UTF-8 body changes {changed}",
        base.entries.len(),
        current.entries.len()
    ))
}

pub fn refresh(config_path: &Path, reference_date: NaiveDate, jobs: usize) -> Result<()> {
    let old_config_bytes =
        fs::read(config_path).with_context(|| format!("read config {}", config_path.display()))?;
    let mut config = load_config(config_path)?;
    let root = config_path.parent().unwrap_or_else(|| Path::new("."));
    let old_manifest = optional_json::<CorpusManifest>(&root.join(&config.output.manifest))?;
    let old_quarantine =
        optional_json::<QuarantineManifest>(&root.join(&config.output.quarantine))?;
    let old_commit = config.upstream.commit.clone();
    let latest_commit = upstream_head(&config.upstream.repository)?;
    config.upstream.commit = latest_commit;
    config.policy.reference_date = reference_date.format("%Y-%m-%d").to_string();
    config.policy.cutoff_year = u16::try_from(reference_date.year() - 96)
        .context("reference date produces an invalid cutoff")?;

    let reader = UpstreamReader::new(Arc::new(config.clone()), Input::Remote)?;
    let metadata = reader
        .read(&config.upstream.metadata_path, MAX_ARCHIVE_BYTES)
        .context("download refreshed metadata snapshot")?;
    config.upstream.metadata_sha256 = sha256(&metadata);
    let csv = extract_metadata_csv(&metadata)?;
    let scan = scan_csv(&config, &csv)?;
    let old_accepted = old_manifest
        .as_ref()
        .map_or(config.policy.expected_accepted_editions, |value| {
            value.entries.len()
        });
    if scan.candidates.len() + config.policy.max_accepted_decrease < old_accepted {
        bail!(
            "candidate count unexpectedly decreased from {old_accepted} to {}; manual policy review required",
            scan.candidates.len()
        );
    }
    config.policy.expected_accepted_editions = scan.candidates.len();
    validate_config(&config)?;
    write_toml(config_path, &config)?;
    if let Err(error) = snapshot(config_path, Input::Remote, jobs) {
        fs::write(config_path, old_config_bytes)
            .with_context(|| format!("restore config {}", config_path.display()))?;
        return Err(error).context("refreshed snapshot failed; corpus.toml was restored");
    }
    let new_manifest: CorpusManifest = read_json(&root.join(&config.output.manifest))?;
    let new_quarantine: QuarantineManifest = read_json(&root.join(&config.output.quarantine))?;
    let report = render_update_report(
        &old_commit,
        &config.upstream.commit,
        old_manifest.as_ref(),
        &new_manifest,
        old_quarantine.as_ref(),
        &new_quarantine,
    );
    fs::write(root.join("corpus/update-report.md"), report)
        .context("write corpus update report")?;
    Ok(())
}

fn upstream_head(repository: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["ls-remote", repository, "HEAD"])
        .output()
        .context("query upstream HEAD")?;
    if !output.status.success() {
        bail!(
            "git ls-remote failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let commit = String::from_utf8(output.stdout)
        .context("upstream ls-remote output is not UTF-8")?
        .split_whitespace()
        .next()
        .context("upstream ls-remote returned no commit")?
        .to_owned();
    validate_commit(&commit)?;
    Ok(commit)
}

fn optional_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    if path.exists() {
        read_json(path).map(Some)
    } else {
        Ok(None)
    }
}

fn write_toml(path: &Path, value: &Config) -> Result<()> {
    let mut text = toml::to_string_pretty(value).context("serialize corpus policy TOML")?;
    text.insert_str(
        0,
        "# Generated provenance is changed only by a reviewed corpus update PR.\n",
    );
    let temporary = path.with_extension("toml.tmp");
    fs::write(&temporary, text)
        .with_context(|| format!("write temporary config {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("replace config {}", path.display()))
}

fn render_update_report(
    old_commit: &str,
    new_commit: &str,
    before: Option<&CorpusManifest>,
    after: &CorpusManifest,
    before_quarantine: Option<&QuarantineManifest>,
    after_quarantine: &QuarantineManifest,
) -> String {
    let before_entries = before
        .map(|manifest| {
            manifest
                .entries
                .iter()
                .map(|entry| (entry.edition_id.as_str(), entry))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let after_entries = after
        .entries
        .iter()
        .map(|entry| (entry.edition_id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let added = after_entries
        .iter()
        .filter(|(id, _)| !before_entries.contains_key(*id))
        .map(|(_, entry)| *entry)
        .collect::<Vec<_>>();
    let removed = before_entries
        .iter()
        .filter(|(id, _)| !after_entries.contains_key(*id))
        .map(|(_, entry)| *entry)
        .collect::<Vec<_>>();
    let changed = after_entries
        .iter()
        .filter_map(|(id, entry)| {
            before_entries
                .get(id)
                .and_then(|before| (before.utf8_sha256 != entry.utf8_sha256).then_some(*entry))
        })
        .collect::<Vec<_>>();
    let before_root = before.map_or_else(|| sha256([]), manifest_root);
    let after_root = manifest_root(after);
    let before_reasons = before_quarantine.map(reason_counts).unwrap_or_default();
    let after_reasons = reason_counts(after_quarantine);
    let mut report = format!(
        "# Corpus snapshot update\n\n- Upstream commit: `{old_commit}` → `{new_commit}`\n- Accepted editions: {} → {}\n- Added: {}\n- Removed: {}\n- UTF-8 body changes: {}\n- Snapshot root: `{before_root}` → `{after_root}`\n\n",
        before.map_or(0, |manifest| manifest.entries.len()),
        after.entries.len(),
        added.len(),
        removed.len(),
        changed.len(),
    );
    report
        .push_str("## Quarantine reason counts\n\n| Reason | Before | After |\n|---|---:|---:|\n");
    let reasons = before_reasons
        .keys()
        .chain(after_reasons.keys())
        .collect::<BTreeSet<_>>();
    for reason in reasons {
        report.push_str(&format!(
            "| `{reason:?}` | {} | {} |\n",
            before_reasons.get(reason).copied().unwrap_or_default(),
            after_reasons.get(reason).copied().unwrap_or_default()
        ));
    }
    append_editions(&mut report, "Added editions", &added);
    append_editions(&mut report, "Removed editions", &removed);
    append_editions(&mut report, "Changed UTF-8 bodies", &changed);
    report.push_str(
        "\nThis report is generated for review. Updates are never merged automatically.\n",
    );
    report
}

fn manifest_root(manifest: &CorpusManifest) -> String {
    let mut material = Vec::new();
    for entry in &manifest.entries {
        material.extend_from_slice(entry.edition_id.as_bytes());
        material.push(b'\n');
        material.extend_from_slice(entry.utf8_sha256.as_bytes());
        material.push(b'\n');
    }
    sha256(material)
}

fn reason_counts(manifest: &QuarantineManifest) -> BTreeMap<RejectionReason, usize> {
    let mut counts = BTreeMap::new();
    for entry in &manifest.entries {
        for reason in &entry.reasons {
            *counts.entry(reason.clone()).or_default() += 1;
        }
    }
    counts
}

fn append_editions(report: &mut String, heading: &str, entries: &[&CorpusEntry]) {
    report.push_str(&format!("\n## {heading}\n\n"));
    if entries.is_empty() {
        report.push_str("None.\n");
        return;
    }
    const MAX_LISTED: usize = 500;
    for entry in entries.iter().take(MAX_LISTED) {
        let title = entry.title.replace(['\r', '\n', '|'], " ");
        report.push_str(&format!(
            "- `{}` {} (`{}`)\n",
            entry.edition_id, title, entry.utf8_sha256
        ));
    }
    if entries.len() > MAX_LISTED {
        report.push_str(&format!(
            "- … {} more entries omitted from this rendered report; inspect manifest diff.\n",
            entries.len() - MAX_LISTED
        ));
    }
}

fn write_sources(root: &Path, relative: &str, sources: &[(String, Vec<u8>)]) -> Result<()> {
    let directory = root.join(relative);
    fs::create_dir_all(&directory)
        .with_context(|| format!("create source directory {}", directory.display()))?;
    let expected = sources
        .iter()
        .map(|(filename, _)| filename.as_str())
        .collect::<BTreeSet<_>>();
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow!("existing source filename is not UTF-8"))?;
        if entry.file_type()?.is_file() && !expected.contains(name.as_str()) {
            fs::remove_file(entry.path())
                .with_context(|| format!("remove stale generated source {name}"))?;
        }
    }
    for (filename, bytes) in sources {
        let path = directory.join(filename);
        let temporary = directory.join(format!(".{filename}.tmp"));
        fs::write(&temporary, bytes)
            .with_context(|| format!("write temporary source {}", temporary.display()))?;
        fs::rename(&temporary, &path)
            .with_context(|| format!("replace generated source {}", path.display()))?;
    }
    Ok(())
}

fn write_json(path: PathBuf, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create output directory {}", parent.display()))?;
    }
    let mut bytes = serde_json::to_vec_pretty(value).context("serialize JSON")?;
    bytes.push(b'\n');
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes)
        .with_context(|| format!("write temporary JSON {}", temporary.display()))?;
    fs::rename(&temporary, &path)
        .with_context(|| format!("replace generated JSON {}", path.display()))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("read JSON {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse JSON {}", path.display()))
}

fn verify_checkout(root: &Path, expected: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .with_context(|| format!("run git in upstream checkout {}", root.display()))?;
    if !output.status.success() {
        bail!("upstream checkout is not a readable Git repository");
    }
    let actual = String::from_utf8(output.stdout)
        .context("upstream commit output is not UTF-8")?
        .trim()
        .to_owned();
    if actual != expected {
        bail!("upstream checkout commit mismatch: expected {expected}, got {actual}");
    }
    Ok(())
}

fn safe_relative_path(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("path must be a non-empty, normalized relative path: {value}");
    }
    Ok(path.to_owned())
}

fn archive_relative_path(value: &str) -> Result<String> {
    let url = validate_official_url(value, ".zip")?;
    let relative = url
        .path()
        .strip_prefix('/')
        .context("archive URL path is not absolute")?;
    safe_relative_path(relative)?;
    Ok(relative.to_owned())
}

fn validate_official_url(value: &str, suffix: &str) -> Result<Url> {
    let url = Url::parse(value).context("parse official URL")?;
    if url.scheme() != "https"
        || url.host_str() != Some("www.aozora.gr.jp")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.path().starts_with("/cards/") && suffix != ".zip"
        || !url.path().ends_with(suffix)
    {
        bail!("URL is not an official Aozora HTTPS {suffix} resource");
    }
    if suffix == ".zip"
        && url.path() != "/index_pages/list_person_all_extended_utf8.zip"
        && !url.path().starts_with("/cards/")
    {
        bail!("ZIP URL is outside the official metadata/cards paths");
    }
    Ok(url)
}

fn valid_work_id(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn validate_commit(value: &str) -> Result<()> {
    if value.len() != 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        bail!("commit must be a lowercase 40-character SHA");
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        bail!("SHA-256 must be 64 lowercase hexadecimal characters");
    }
    Ok(())
}

#[must_use]
pub fn make_edition_id(work_id: &str, archive_url: &str) -> String {
    format!(
        "{work_id}-{}",
        &sha256(format!("{work_id}\n{archive_url}"))[..16]
    )
}

#[must_use]
pub fn sha256(bytes: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes.as_ref());
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{make_edition_id, safe_relative_path, sha256, validate_official_url};

    #[test]
    fn sha256_is_lowercase_and_stable() {
        let digest = sha256("abc");
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(digest.len(), 64);
    }

    #[test]
    fn edition_identity_depends_on_work_and_archive_url() {
        assert_eq!(
            make_edition_id(
                "000001",
                "https://www.aozora.gr.jp/cards/000001/files/1.zip"
            ),
            "000001-f97c41768069bdeb"
        );
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(safe_relative_path("../cards/file.zip").is_err());
    }

    #[test]
    fn rejects_non_official_archive_url() {
        assert!(validate_official_url("https://example.com/file.zip", ".zip").is_err());
    }
}
