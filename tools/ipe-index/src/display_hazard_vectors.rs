//! Pins `tests/display_hazard_vectors.json` to the Unicode Character Database.
//!
//! The file is the review-display hazard set code-review's `SafeText.isHazard`
//! is tested against: `Cc`, `Cf`, `Zl`, `Zp`, `Zs` except U+0020,
//! `Default_Ignorable_Code_Point`, and U+2800 BRAILLE PATTERN BLANK. It must
//! also contain every range of the runtime's `log_hazard_ranges.json`.

#[cfg(test)]
mod tests {
    use icu_properties::CodePointSetData;
    use icu_properties::props::DefaultIgnorableCodePoint;
    use unicode_general_category::{GeneralCategory, get_general_category};

    const DISPLAY_VECTORS: &str = include_str!("../tests/display_hazard_vectors.json");
    const LOG_RANGES: &str = include_str!("../../../src/runtime/rust/tests/log_hazard_ranges.json");

    /// The `lo`/`hi` pairs of a ranges file, checked sorted and disjoint.
    fn ranges(text: &str) -> Vec<(u32, u32)> {
        let rows: Vec<serde_json::Value> = serde_json::from_str(text).unwrap();
        let pairs: Vec<(u32, u32)> = rows
            .iter()
            .map(|row| {
                let lo = u32::try_from(row["lo"].as_u64().unwrap()).unwrap();
                let hi = u32::try_from(row["hi"].as_u64().unwrap()).unwrap();
                assert!(lo <= hi, "range {lo}..={hi} is inverted");
                (lo, hi)
            })
            .collect();
        assert!(
            pairs.windows(2).all(|w| w[0].1 < w[1].0),
            "ranges are not sorted and disjoint"
        );
        pairs
    }

    fn contains(ranges: &[(u32, u32)], code: u32) -> bool {
        ranges.iter().any(|&(lo, hi)| lo <= code && code <= hi)
    }

    /// The target set, computed from the UCD tables.
    fn in_target(c: char) -> bool {
        let invisible_category = match get_general_category(c) {
            GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator => true,
            GeneralCategory::SpaceSeparator => c != ' ',
            _ => false,
        };
        invisible_category
            || CodePointSetData::new::<DefaultIgnorableCodePoint>().contains(c)
            || c == '\u{2800}'
    }

    // Every target code point is in the fixture; every fixture code point is
    // in the target set or unassigned (a later Unicode may assign it).
    #[test]
    fn display_vectors_match_the_ucd() {
        let display = ranges(DISPLAY_VECTORS);
        assert!(display.len() >= 20, "the display vector file lost rows");
        for row in serde_json::from_str::<Vec<serde_json::Value>>(DISPLAY_VECTORS).unwrap() {
            assert!(
                row["why"].as_str().is_some_and(|w| !w.is_empty()),
                "row {row} has no why"
            );
        }
        for c in char::MIN..=char::MAX {
            let listed = contains(&display, u32::from(c));
            if in_target(c) {
                assert!(listed, "{c:?} is a display hazard the fixture omits");
            }
            if listed {
                assert!(
                    in_target(c) || matches!(get_general_category(c), GeneralCategory::Unassigned),
                    "{c:?} is in the fixture but is not a display hazard"
                );
            }
        }
    }

    // The display set is a superset of the runtime's log-hazard set.
    #[test]
    fn log_fixture_is_contained_in_display_vectors() {
        let display = ranges(DISPLAY_VECTORS);
        let log = ranges(LOG_RANGES);
        assert!(log.len() >= 20, "the runtime log-hazard file lost rows");
        for (lo, hi) in log {
            for code in lo..=hi {
                assert!(
                    contains(&display, code),
                    "log hazard U+{code:04X} is not a display hazard"
                );
            }
        }
    }
}
