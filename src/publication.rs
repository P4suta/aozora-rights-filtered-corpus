use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::model::RejectionReason;

static GREGORIAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?P<year>(?:1[0-9]{3}|20[0-9]{2}))").expect("valid regex"));
static ERA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<era>明治|大正|昭和|平成|令和)(?P<year>元|[0-9０-９一二三四五六七八九十百]+)年")
        .expect("valid regex")
});
static ERA_PAIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?P<gregorian>(?:1[0-9]{3}|20[0-9]{2}))（(?P<era>明治|大正|昭和|平成|令和)(?P<year>元|[0-9０-９一二三四五六七八九十百]+)）年",
    )
    .expect("valid regex")
});
static SHORT_RANGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9０-９]{2,4}[〜～－—-][0-9０-９]{1,3}年").expect("valid regex")
});
static MALFORMED_MONTH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"）月[0-9０-９]+月").expect("valid regex"));

const AMBIGUOUS_MARKERS: &[&str] = &[
    "不明",
    "未詳",
    "推定",
    "推測",
    "頃",
    "ころ",
    "前後",
    "年代",
    "か？",
    "か?",
    "（？）",
    "(?)",
    "翌年",
    "前年",
    "同年",
];

pub fn parse(raw: &str, cutoff: u16) -> Result<Vec<u16>, RejectionReason> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(RejectionReason::FirstPublicationMissing);
    }
    if value != raw {
        return Err(RejectionReason::FirstPublicationUnparsed);
    }
    if AMBIGUOUS_MARKERS
        .iter()
        .any(|marker| value.contains(marker))
        || SHORT_RANGE.is_match(value)
        || MALFORMED_MONTH.is_match(value)
    {
        return Err(RejectionReason::FirstPublicationAmbiguous);
    }

    for captures in ERA_PAIR.captures_iter(value) {
        let gregorian = captures["gregorian"]
            .parse::<u16>()
            .map_err(|_| RejectionReason::FirstPublicationUnparsed)?;
        let era_year = era_to_gregorian(&captures["era"], &captures["year"])
            .ok_or(RejectionReason::FirstPublicationUnparsed)?;
        if gregorian != era_year {
            return Err(RejectionReason::FirstPublicationUnparsed);
        }
    }

    let mut years = BTreeSet::new();
    for captures in GREGORIAN.captures_iter(value) {
        let year = captures["year"]
            .parse::<u16>()
            .map_err(|_| RejectionReason::FirstPublicationUnparsed)?;
        years.insert(year);
    }
    for captures in ERA.captures_iter(value) {
        years.insert(
            era_to_gregorian(&captures["era"], &captures["year"])
                .ok_or(RejectionReason::FirstPublicationUnparsed)?,
        );
    }
    if years.is_empty() {
        return Err(RejectionReason::FirstPublicationUnparsed);
    }
    if years.iter().any(|year| *year > cutoff) {
        return Err(RejectionReason::FirstPublicationAfterCutoff);
    }
    Ok(years.into_iter().collect())
}

fn era_to_gregorian(era: &str, value: &str) -> Option<u16> {
    let year = if value == "元" {
        1
    } else {
        parse_japanese_number(value)?
    };
    let base: u16 = match era {
        "明治" => 1867,
        "大正" => 1911,
        "昭和" => 1925,
        "平成" => 1988,
        "令和" => 2018,
        _ => return None,
    };
    base.checked_add(year)
}

fn parse_japanese_number(value: &str) -> Option<u16> {
    let ascii = value
        .chars()
        .map(|character| match character {
            '０' => '0',
            '１' => '1',
            '２' => '2',
            '３' => '3',
            '４' => '4',
            '５' => '5',
            '６' => '6',
            '７' => '7',
            '８' => '8',
            '９' => '9',
            other => other,
        })
        .collect::<String>();
    if ascii.bytes().all(|byte| byte.is_ascii_digit()) {
        return ascii.parse().ok();
    }
    let digit = |character| match character {
        '一' => Some(1_u16),
        '二' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        _ => None,
    };
    let mut total = 0_u16;
    let mut pending = 0_u16;
    for character in ascii.chars() {
        match character {
            '十' => {
                total = total.checked_add(if pending == 0 { 10 } else { pending * 10 })?;
                pending = 0;
            }
            '百' => {
                total = total.checked_add(if pending == 0 { 100 } else { pending * 100 })?;
                pending = 0;
            }
            _ => pending = digit(character)?,
        }
    }
    total.checked_add(pending)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::parse;
    use crate::model::RejectionReason;

    #[test]
    fn parses_gregorian_serial_and_matching_era() {
        assert_eq!(
            parse("「文章世界」1929（昭和4）年1月～1930（昭和5）年3月", 1930),
            Ok(vec![1929, 1930])
        );
    }

    #[test]
    fn parses_era_only_and_kanji_year() {
        assert_eq!(parse("「太陽」明治四十五年一月", 1930), Ok(vec![1912]));
    }

    #[test]
    fn rejects_boundary_plus_one() {
        assert_eq!(
            parse("1931（昭和6）年1月", 1930),
            Err(RejectionReason::FirstPublicationAfterCutoff)
        );
    }

    #[test]
    fn rejects_ambiguous_short_range() {
        assert_eq!(
            parse("1929～30年", 1930),
            Err(RejectionReason::FirstPublicationAmbiguous)
        );
    }

    #[test]
    fn rejects_mismatched_era_pair() {
        assert_eq!(
            parse("1930（昭和4）年", 1930),
            Err(RejectionReason::FirstPublicationUnparsed)
        );
    }

    #[test]
    fn rejects_relative_year_and_malformed_month() {
        for value in ["1913（大正2）年12月～翌年9月", "1923（大正12）月1月"] {
            assert_eq!(
                parse(value, 1930),
                Err(RejectionReason::FirstPublicationAmbiguous)
            );
        }
    }

    #[test]
    fn rejects_surrounding_whitespace_without_normalizing_evidence() {
        assert_eq!(
            parse(" 1929（昭和4）年", 1930),
            Err(RejectionReason::FirstPublicationUnparsed)
        );
    }
}
