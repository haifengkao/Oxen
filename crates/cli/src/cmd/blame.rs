use async_trait::async_trait;
use clap::{Arg, ArgMatches, Command};
use liboxen::error::OxenError;
use liboxen::model::{Commit, LocalRepository};
use liboxen::repositories;
use similar::{Algorithm, DiffTag, TextDiff as SimilarTextDiff};
use std::ops::Range;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use tokio_stream::StreamExt;

use crate::cmd::RunCmd;

pub const NAME: &str = "blame";
pub struct BlameCmd;

#[derive(Clone, Debug)]
struct ActiveLine {
    position: usize,
    head_index: usize,
}

#[derive(Clone, Debug)]
struct BlameLine {
    line_number: usize,
    commit: Commit,
    content: String,
}

#[async_trait]
impl RunCmd for BlameCmd {
    fn name(&self) -> &str {
        NAME
    }

    fn args(&self) -> Command {
        Command::new(NAME)
            .about("Show what commit last changed each line of a file.")
            .arg(
                Arg::new("revision")
                    .long("revision")
                    .short('r')
                    .help("The branch, commit id, or HEAD revision to blame from.")
                    .default_value("HEAD")
                    .action(clap::ArgAction::Set),
            )
            .arg(
                Arg::new("json")
                    .long("json")
                    .help("Print machine-readable JSON.")
                    .action(clap::ArgAction::SetTrue),
            )
            .arg(Arg::new("path").required(true).value_name("path"))
    }

    async fn run(&self, args: &ArgMatches) -> Result<(), OxenError> {
        let repo = LocalRepository::from_current_dir()?;
        let revision = args
            .get_one::<String>("revision")
            .expect("Must supply revision");
        let path = args.get_one::<String>("path").expect("Must supply path");
        let repo_path = normalize_repo_path(&repo, PathBuf::from(path))?;
        let lines = blame_file(&repo, revision, &repo_path).await?;

        if args.get_flag("json") {
            println!(
                "{}",
                serde_json::to_string(&blame_json_value(&repo_path, &lines))?
            );
            return Ok(());
        }

        for line in lines {
            println!(
                "{} ({:<16} {:>4}) {}",
                short_commit_id(&line.commit.id),
                line.commit.author,
                line.line_number,
                line.content
            );
        }

        Ok(())
    }
}

fn normalize_repo_path(
    repo: &LocalRepository,
    path: impl AsRef<Path>,
) -> Result<PathBuf, OxenError> {
    let path = path.as_ref();

    if path.is_absolute() {
        return liboxen::util::fs::path_relative_to_dir(path, &repo.path);
    }

    let current_dir = std::env::current_dir()?;
    liboxen::util::fs::path_relative_to_dir(current_dir.join(path), &repo.path)
}

async fn blame_file(
    repo: &LocalRepository,
    revision: &str,
    path: &Path,
) -> Result<Vec<BlameLine>, OxenError> {
    let head = repositories::revisions::get(repo, revision)?
        .ok_or_else(|| OxenError::RevisionNotFound(revision.into()))?;
    let Some(head_content) = read_file_at_commit(repo, &head, path).await? else {
        return Err(OxenError::entry_does_not_exist_in_commit(path, &head.id));
    };
    let head_lines = split_lines(&head_content);
    let mut active_lines = head_lines
        .iter()
        .enumerate()
        .map(|(position, _)| ActiveLine {
            position,
            head_index: position,
        })
        .collect::<Vec<_>>();
    let mut annotations = vec![None::<Commit>; head_lines.len()];
    let mut skip = 0;

    while !active_lines.is_empty() {
        let commits =
            repositories::commits::list_from_without_count_cache(repo, &head.id, skip, 100)?;
        if commits.is_empty() {
            break;
        }

        for commit in &commits {
            if active_lines.is_empty() {
                break;
            }
            blame_commit(repo, path, commit, &mut active_lines, &mut annotations).await?;
        }

        skip += commits.len();
    }

    head_lines
        .into_iter()
        .enumerate()
        .map(|(idx, content)| {
            let Some(commit) = annotations[idx].clone() else {
                return Err(OxenError::basic_str(format!(
                    "Could not resolve blame for line {} in {path:?}",
                    idx + 1
                )));
            };
            Ok(BlameLine {
                line_number: idx + 1,
                commit,
                content,
            })
        })
        .collect()
}

async fn blame_commit(
    repo: &LocalRepository,
    path: &Path,
    commit: &Commit,
    active_lines: &mut Vec<ActiveLine>,
    annotations: &mut [Option<Commit>],
) -> Result<(), OxenError> {
    let Some(commit_content) = read_file_at_commit(repo, commit, path).await? else {
        return Ok(());
    };
    let parent_content = match commit.parent_ids.first() {
        Some(parent_id) => {
            let parent = repositories::commits::get_by_id(repo, parent_id)?
                .ok_or_else(|| OxenError::commit_id_does_not_exist(parent_id))?;
            read_file_at_commit(repo, &parent, path).await?
        }
        None => None,
    };
    let commit_lines = split_lines(&commit_content);
    let parent_lines = parent_content
        .as_deref()
        .map(split_lines)
        .unwrap_or_default();

    if commit_lines == parent_lines {
        return Ok(());
    }

    let parent_line_refs = parent_lines.iter().map(String::as_str).collect::<Vec<_>>();
    let commit_line_refs = commit_lines.iter().map(String::as_str).collect::<Vec<_>>();
    let diff = SimilarTextDiff::configure()
        .algorithm(Algorithm::Patience)
        .diff_slices(&parent_line_refs, &commit_line_refs);
    let mut next_active = Vec::new();

    for op in diff.ops() {
        let new_range = op.new_range();
        match op.tag() {
            DiffTag::Equal => {
                for line in active_lines_in_range(active_lines, new_range.clone()) {
                    next_active.push(ActiveLine {
                        position: op.old_range().start + (line.position - new_range.start),
                        head_index: line.head_index,
                    });
                }
            }
            DiffTag::Insert | DiffTag::Replace => {
                for line in active_lines_in_range(active_lines, new_range) {
                    if annotations[line.head_index].is_none() {
                        annotations[line.head_index] = Some(commit.clone());
                    }
                }
            }
            DiffTag::Delete => {}
        }
    }

    *active_lines = next_active;
    Ok(())
}

fn active_lines_in_range(active_lines: &[ActiveLine], range: Range<usize>) -> Vec<ActiveLine> {
    active_lines
        .iter()
        .filter(|line| range.contains(&line.position))
        .cloned()
        .collect()
}

async fn read_file_at_commit(
    repo: &LocalRepository,
    commit: &Commit,
    path: &Path,
) -> Result<Option<String>, OxenError> {
    let Some(file_node) = repositories::tree::get_file_by_path(repo, commit, path)? else {
        return Ok(None);
    };
    let version_store = repo.version_store()?;
    let mut stream = version_store
        .get_version_stream(&file_node.hash().to_string())
        .await?;
    let mut data = Vec::new();
    while let Some(chunk) = stream.next().await {
        data.extend(chunk?);
    }
    String::from_utf8(data)
        .map(Some)
        .map_err(|e| OxenError::basic_str(format!("Cannot blame non-UTF-8 file {path:?}: {e}")))
}

fn split_lines(content: &str) -> Vec<String> {
    if content.is_empty() {
        return Vec::new();
    }

    let mut lines = content
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect::<Vec<_>>();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

fn blame_json_value(path: &Path, lines: &[BlameLine]) -> serde_json::Value {
    let lines: Vec<serde_json::Value> = lines
        .iter()
        .map(|line| {
            serde_json::json!({
                "line_number": line.line_number,
                "content": line.content,
                "commit": {
                    "commit_id": line.commit.id,
                    "parent_ids": line.commit.parent_ids,
                    "message": line.commit.message,
                    "author": line.commit.author,
                    "email": line.commit.email,
                    "timestamp": line.commit.timestamp.format(&Rfc3339).expect("commit timestamp must format as RFC3339"),
                },
            })
        })
        .collect();

    serde_json::json!({
        "path": path.to_string_lossy(),
        "lines": lines,
    })
}

fn short_commit_id(commit_id: &str) -> &str {
    commit_id.get(0..8).unwrap_or(commit_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use liboxen::{repositories, test, util};

    #[tokio::test]
    async fn blame_file_assigns_each_current_line_to_the_commit_that_changed_it()
    -> Result<(), OxenError> {
        test::run_empty_local_repo_test_async(|repo| async move {
            let relative_path = PathBuf::from("notes.txt");
            let path = repo.path.join(&relative_path);

            util::fs::write_to_path(&path, "alpha\nbeta\n")?;
            repositories::add(&repo, &path).await?;
            let first_commit = repositories::commit(&repo, "add notes")?;

            util::fs::write_to_path(&path, "alpha\nbeta\ngamma\n")?;
            repositories::add(&repo, &path).await?;
            let second_commit = repositories::commit(&repo, "append gamma")?;

            util::fs::write_to_path(&path, "alpha changed\nbeta\ngamma\n")?;
            repositories::add(&repo, &path).await?;
            let third_commit = repositories::commit(&repo, "change alpha")?;

            let lines = blame_file(&repo, "main", &relative_path).await?;

            assert_eq!(
                lines
                    .iter()
                    .map(|line| (
                        line.line_number,
                        line.content.as_str(),
                        line.commit.id.as_str()
                    ))
                    .collect::<Vec<_>>(),
                vec![
                    (1, "alpha changed", third_commit.id.as_str()),
                    (2, "beta", first_commit.id.as_str()),
                    (3, "gamma", second_commit.id.as_str()),
                ]
            );

            Ok(())
        })
        .await
    }

    #[test]
    fn blame_json_includes_line_commit_metadata() {
        let commit = Commit {
            id: "abc123".to_string(),
            parent_ids: vec!["parent123".to_string()],
            message: "Add line".to_string(),
            author: "Ox".to_string(),
            email: "ox@example.com".to_string(),
            timestamp: time::OffsetDateTime::from_unix_timestamp(0).unwrap(),
        };
        let json = blame_json_value(
            Path::new("notes.txt"),
            &[BlameLine {
                line_number: 1,
                commit,
                content: "hello".to_string(),
            }],
        );

        assert_eq!(
            json,
            serde_json::json!({
                "path": "notes.txt",
                "lines": [
                    {
                        "line_number": 1,
                        "content": "hello",
                        "commit": {
                            "commit_id": "abc123",
                            "parent_ids": ["parent123"],
                            "message": "Add line",
                            "author": "Ox",
                            "email": "ox@example.com",
                            "timestamp": "1970-01-01T00:00:00Z"
                        }
                    }
                ]
            })
        );
    }

    #[test]
    fn blame_args_accept_git_style_path_with_or_without_separator() {
        let with_separator = BlameCmd
            .args()
            .try_get_matches_from(["blame", "--json", "--", "notes.txt"])
            .unwrap();
        let without_separator = BlameCmd
            .args()
            .try_get_matches_from(["blame", "--json", "notes.txt"])
            .unwrap();

        assert_eq!(
            with_separator.get_one::<String>("path").unwrap(),
            "notes.txt"
        );
        assert_eq!(
            without_separator.get_one::<String>("path").unwrap(),
            "notes.txt"
        );
    }
}
