use crate::error::OxenError;
use crate::model::diff::change_type::ChangeType;
use crate::model::diff::text_diff::LineDiff;
use crate::model::diff::text_diff::TextDiff;

use similar::{Algorithm, ChangeTag, TextDiff as SimilarTextDiff};
use std::path::PathBuf;
use std::time::Duration;

const CONTEXT_LINES: usize = 3;
const DIFF_TIMEOUT: Duration = Duration::from_millis(500);

fn strip_line_ending(line: &str) -> &str {
    if let Some(line) = line.strip_suffix("\r\n") {
        line
    } else if let Some(line) = line.strip_suffix('\n') {
        line
    } else if let Some(line) = line.strip_suffix('\r') {
        line
    } else {
        line
    }
}

pub fn diff(
    original_data: Option<String>,
    version_file_1: Option<PathBuf>,
    compare_data: Option<String>,
    version_file_2: Option<PathBuf>,
) -> Result<TextDiff, OxenError> {
    let mut result = TextDiff {
        filename1: version_file_1
            .clone()
            .map(|p| p.to_string_lossy().to_string()),
        filename2: version_file_2
            .clone()
            .map(|p| p.to_string_lossy().to_string()),
        ..Default::default()
    };

    let original_data = original_data.unwrap_or_default();
    let compare_data = compare_data.unwrap_or_default();

    let diff = SimilarTextDiff::configure()
        .algorithm(Algorithm::Patience)
        .timeout(DIFF_TIMEOUT)
        .diff_lines(&original_data, &compare_data);

    if diff.ratio() == 1.0 {
        log::debug!("No changes detected, returning empty TextDiff.");
        return Ok(result);
    }

    for (idx, group) in diff.grouped_ops(CONTEXT_LINES).iter().enumerate() {
        if idx > 0 {
            result.lines.push(LineDiff {
                modification: ChangeType::Unchanged,
                text: "...".to_string(),
            });
        }

        for op in group {
            for change in diff.iter_changes(op) {
                let modification = match change.tag() {
                    ChangeTag::Equal => ChangeType::Unchanged,
                    ChangeTag::Delete => ChangeType::Removed,
                    ChangeTag::Insert => ChangeType::Added,
                };
                result.lines.push(LineDiff {
                    modification,
                    text: strip_line_ending(change.value()).to_string(),
                });
            }
        }
    }

    log::debug!(
        "contextual_diff returning result with {} lines",
        result.lines.len()
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn large_shifted_text_diff_returns_without_timing_out() {
        let mut original = String::new();
        let mut compare = String::new();

        for idx in 0..20_000 {
            original.push_str(&format!("line-{idx}\n"));
            compare.push_str(&format!("line-{}\n", idx + 1));
        }
        compare.push_str("tail\n");

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let diff = diff(Some(original), None, Some(compare), None);
            tx.send(diff).expect("test receiver should still be open");
        });

        let diff = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("large text diff should not hang or exhaust memory")
            .expect("large text diff should succeed");

        assert!(
            diff.lines
                .iter()
                .any(|line| line.modification == ChangeType::Added && line.text == "tail"),
            "diff should include the inserted tail line"
        );
    }
}
