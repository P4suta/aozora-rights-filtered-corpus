use aozora_rights_filtered_corpus::generator::scan_csv;
use aozora_rights_filtered_corpus::model::{
    Config, OutputConfig, PolicyConfig, RejectionReason, UpstreamConfig,
};

const HEADER: &str = "作品ID,作品名,作品名読み,初出,作品著作権フラグ,図書カードURL,人物ID,姓,名,役割フラグ,人物著作権フラグ,テキストファイルURL,テキストファイル符号化方式\n";

fn config() -> Config {
    Config {
        policy: PolicyConfig {
            reference_date: "2026-08-02".into(),
            cutoff_year: 1930,
            expected_accepted_editions: 1,
            max_accepted_decrease: 0,
        },
        upstream: UpstreamConfig {
            repository: "https://github.com/aozorabunko/aozorabunko".into(),
            commit: "a".repeat(40),
            metadata_path: "index_pages/list_person_all_extended_utf8.zip".into(),
            metadata_url: "https://www.aozora.gr.jp/index_pages/list_person_all_extended_utf8.zip"
                .into(),
            metadata_sha256: "b".repeat(64),
        },
        output: OutputConfig {
            manifest: "corpus/manifest.json".into(),
            quarantine: "corpus/quarantine.json".into(),
            sources: "corpus/sources".into(),
        },
    }
}

fn row(person: &str, role: &str, person_copyright: &str, first: &str) -> String {
    format!(
        "000001,作品,さくひん,{first},なし,https://www.aozora.gr.jp/cards/000001/card1.html,{person},姓,名,{role},{person_copyright},https://www.aozora.gr.jp/cards/000001/files/1.zip,ShiftJIS\n"
    )
}

#[test]
fn accepts_all_rights_none_contributors_and_deduplicates_rows() {
    let bytes = format!(
        "{HEADER}{}{}{}",
        row("000001", "著者", "なし", "1930（昭和5）年"),
        row("000002", "翻訳者", "なし", "1930（昭和5）年"),
        row("000002", "翻訳者", "なし", "1930（昭和5）年")
    );
    let report = scan_csv(&config(), bytes.as_bytes()).expect("scan fixture");
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].contributors.len(), 2);
    assert!(report.quarantine.is_empty());
}

#[test]
fn rejects_when_any_translator_is_still_protected() {
    let bytes = format!(
        "{HEADER}{}{}",
        row("000001", "著者", "なし", "1929年"),
        row("000002", "翻訳者", "あり", "1929年")
    );
    let report = scan_csv(&config(), bytes.as_bytes()).expect("scan fixture");
    assert!(report.candidates.is_empty());
    assert_eq!(
        report.quarantine[0].reasons,
        vec![RejectionReason::ContributorCopyright]
    );
}

#[test]
fn rejects_empty_and_ambiguous_first_publication() {
    for (first, reason) in [
        ("", RejectionReason::FirstPublicationMissing),
        ("1929年頃", RejectionReason::FirstPublicationAmbiguous),
    ] {
        let bytes = format!("{HEADER}{}", row("000001", "著者", "なし", first));
        let report = scan_csv(&config(), bytes.as_bytes()).expect("scan fixture");
        assert_eq!(report.quarantine[0].reasons, vec![reason]);
    }
}
