use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub policy: PolicyConfig,
    pub upstream: UpstreamConfig,
    pub output: OutputConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyConfig {
    pub reference_date: String,
    pub cutoff_year: u16,
    pub expected_accepted_editions: usize,
    pub max_accepted_decrease: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamConfig {
    pub repository: String,
    pub commit: String,
    pub metadata_path: String,
    pub metadata_url: String,
    pub metadata_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    pub manifest: String,
    pub quarantine: String,
    pub sources: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CsvRow {
    #[serde(rename = "作品ID")]
    pub work_id: String,
    #[serde(rename = "作品名")]
    pub title: String,
    #[serde(rename = "作品名読み")]
    pub reading: String,
    #[serde(rename = "初出")]
    pub first_publication: String,
    #[serde(rename = "作品著作権フラグ")]
    pub work_copyright: String,
    #[serde(rename = "図書カードURL")]
    pub card_url: String,
    #[serde(rename = "人物ID")]
    pub contributor_id: String,
    #[serde(rename = "姓")]
    pub surname: String,
    #[serde(rename = "名")]
    pub given_name: String,
    #[serde(rename = "役割フラグ")]
    pub role: String,
    #[serde(rename = "人物著作権フラグ")]
    pub contributor_copyright: String,
    #[serde(rename = "テキストファイルURL")]
    pub archive_url: String,
    #[serde(rename = "テキストファイル符号化方式")]
    pub encoding: String,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Contributor {
    pub role: String,
    pub name: String,
    pub copyright: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FirstPublication {
    pub raw: String,
    pub years: Vec<u16>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct CorpusInfo {
    pub repository: String,
    pub commit: String,
    pub reference_date: String,
    pub cutoff_year: u16,
    pub metadata_url: String,
    pub metadata_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct CorpusEntry {
    pub edition_id: String,
    pub work_id: String,
    pub title: String,
    pub reading: String,
    pub copyright: String,
    pub contributors: Vec<Contributor>,
    pub first_publication: FirstPublication,
    pub card_url: String,
    pub archive_url: String,
    pub archive_filename: String,
    pub utf8_filename: String,
    pub upstream_commit: String,
    pub csv_sha256: String,
    pub archive_sha256: String,
    pub utf8_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct CorpusManifest {
    pub schema_version: u32,
    pub corpus: CorpusInfo,
    pub entries: Vec<CorpusEntry>,
}

#[derive(Clone, Debug)]
pub struct EditionDraft {
    pub edition_id: String,
    pub work_id: String,
    pub title: String,
    pub reading: String,
    pub contributors: Vec<Contributor>,
    pub first_publication: FirstPublication,
    pub card_url: String,
    pub archive_url: String,
    pub encoding: String,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionReason {
    ArchiveDownloadFailed,
    ArchiveHashMismatch,
    ArchiveTooLarge,
    ArchiveUnsafePath,
    ArchiveWithoutSingleText,
    ContributorCopyright,
    DuplicateEdition,
    EmptyContributor,
    FirstPublicationAfterCutoff,
    FirstPublicationAmbiguous,
    FirstPublicationMissing,
    FirstPublicationUnparsed,
    InvalidArchiveUrl,
    InvalidCardUrl,
    InvalidWorkId,
    MetadataMismatch,
    SourceDecodeFailed,
    SourceHashMismatch,
    UnsupportedEncoding,
    WorkCopyright,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct QuarantineEntry {
    pub edition_id: Option<String>,
    pub work_id: String,
    pub archive_url: String,
    pub reasons: Vec<RejectionReason>,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct QuarantineSummary {
    pub csv_rows: usize,
    pub edition_groups: usize,
    pub candidate_editions: usize,
    pub accepted_editions: usize,
    pub quarantined_editions: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct QuarantineManifest {
    pub schema_version: u32,
    pub reference_date: String,
    pub cutoff_year: u16,
    pub upstream_commit: String,
    pub metadata_sha256: String,
    pub summary: QuarantineSummary,
    pub entries: Vec<QuarantineEntry>,
}
