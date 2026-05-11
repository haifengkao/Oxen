use async_trait::async_trait;
use clap::{Arg, ArgMatches, Command};

use glob::glob;
use liboxen::core::oxenignore;
use liboxen::error::OxenError;
use liboxen::model::staged_data::StagedDataOpts;
use liboxen::model::{Branch, Commit, LocalRepository, StagedData, StagedEntryStatus};
use liboxen::opts::GlobOpts;
use liboxen::repositories;
use liboxen::util;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::helpers::check_repo_migration_needed;

use crate::cmd::RunCmd;
pub const NAME: &str = "status";
pub struct StatusCmd;

#[async_trait]
impl RunCmd for StatusCmd {
    fn name(&self) -> &str {
        NAME
    }
    fn args(&self) -> Command {
        Command::new(NAME)
            .about("View the repository status, including staged, untracked, modified, and removed files")
            .arg(
                Arg::new("skip")
                    .long("skip")
                    .short('s')
                    .help("Allows you to skip and paginate through the file list preview.")
                    .default_value("0")
                    .action(clap::ArgAction::Set),
            )
            .arg(
                Arg::new("limit")
                    .long("limit")
                    .short('l')
                    .help("Allows you to view more file list preview.")
                    .default_value("10")
                    .action(clap::ArgAction::Set),
            )
            .arg(
                Arg::new("ignore")
                    .long("ignore")
                    .short('i')
                    .help("Ignore files at this path.")
                    .action(clap::ArgAction::Set)
                    .required(false),
            )
            .arg(
                Arg::new("print_all")
                    .long("print_all")
                    .short('a')
                    .help("If present, does not truncate the output of status at all.")
                    .action(clap::ArgAction::SetTrue),
            )
            .arg(
                Arg::new("json")
                    .long("json")
                    .help("If present, prints repository status as machine-readable JSON.")
                    .action(clap::ArgAction::SetTrue),
            )
            .arg(
                Arg::new("paths")
                    .num_args(0..)
                    .trailing_var_arg(true)  // Collect all remaining args as paths
                    .help("Specify one or more paths")
            )
    }

    async fn run(&self, args: &ArgMatches) -> Result<(), OxenError> {
        let skip = args
            .get_one::<String>("skip")
            .expect("Must supply skip")
            .parse::<usize>()
            .expect("skip must be a valid integer.");
        let limit = args
            .get_one::<String>("limit")
            .expect("Must supply limit")
            .parse::<usize>()
            .expect("limit must be a valid integer.");
        let print_all = args.get_flag("print_all");

        let repository = LocalRepository::from_current_dir()?;
        check_repo_migration_needed(&repository)?;

        let paths: Vec<PathBuf> = args
            .get_many::<String>("paths")
            .map(|vals| vals.map(PathBuf::from).collect())
            .unwrap_or_default();
        let paths = if paths.is_empty() {
            vec![repository.path.clone()]
        } else {
            parse_status_paths(&repository, &paths).await?
        };

        let is_remote = false;
        let opts = StagedDataOpts {
            paths,
            skip,
            limit,
            print_all,
            is_remote,
            ignore: parse_ignore_files(args.get_one::<String>("ignore")),
        };
        log::debug!("status opts: {opts:?}");

        let repo_status = repositories::status::status_from_opts(&repository, &opts).await?;
        let current_branch = repositories::branches::current_branch(&repository)?;
        let head = repositories::commits::head_commit_maybe(&repository)?;

        if args.get_flag("json") {
            let json = status_json_value(
                &repo_status,
                current_branch.as_ref(),
                head.as_ref(),
                Some(&repository),
            )
            .await?;
            println!("{}", serde_json::to_string(&json)?);
            return Ok(());
        }

        if let Some(current_branch) = current_branch {
            println!(
                "On branch {} -> {}\n",
                current_branch.name, current_branch.commit_id
            );
        } else if let Some(head) = head {
            println!(
                "You are in 'detached HEAD' state.\nHEAD is now at {} {}\n",
                head.id, head.message
            );
        }

        repo_status.print_with_params(&opts);

        Ok(())
    }
}

async fn parse_status_paths(
    repository: &LocalRepository,
    paths: &[PathBuf],
) -> Result<Vec<PathBuf>, OxenError> {
    let paths: Vec<PathBuf> = paths
        .iter()
        .map(|path| repository.path.join(path))
        .collect();
    let glob_opts = GlobOpts {
        paths,
        staged_db: false,
        merkle_tree: true,
        working_dir: true,
        walk_dirs: false,
    };
    let head_commit = repositories::commits::head_commit_maybe(repository)?;

    let expanded_paths = util::glob::parse_glob_paths(&glob_opts, Some(repository)).await?;
    let mut parsed_paths: Vec<PathBuf> = Vec::new();

    let mut filtered_existing: Vec<PathBuf> = expanded_paths
        .iter()
        .filter(|path| path.exists())
        .cloned()
        .collect();
    parsed_paths.append(&mut filtered_existing);

    if let Some(commit) = head_commit {
        for path in expanded_paths.iter() {
            if path.exists() {
                continue;
            }
            let path_in_repo = util::fs::path_relative_to_dir(path, &repository.path)?;
            if repositories::tree::get_node_by_path(repository, &commit, &path_in_repo)?.is_some() {
                parsed_paths.push(path.clone());
            }
        }
    }

    parsed_paths.sort();
    parsed_paths.dedup();

    Ok(parsed_paths)
}

async fn status_json_value(
    repo_status: &StagedData,
    current_branch: Option<&Branch>,
    head: Option<&Commit>,
    repo: Option<&LocalRepository>,
) -> Result<serde_json::Value, OxenError> {
    let mut staged = Vec::new();
    let mut working = Vec::new();

    for (path, staged_dirs) in repo_status.staged_dirs.paths.iter() {
        if path == Path::new("") {
            continue;
        }
        for staged_dir in staged_dirs {
            staged.push(status_entry_json(
                &staged_dir.path,
                staged_entry_status_str(&staged_dir.status),
                true,
                Some(staged_dir.num_files_staged),
            ));
        }
    }

    let mut staged_files: Vec<_> = repo_status.staged_files.iter().collect();
    staged_files.sort_by_key(|(path, _)| *path);
    for (path, entry) in staged_files {
        staged.push(status_entry_json(
            path,
            staged_entry_status_str(&entry.status),
            false,
            None,
        ));
    }

    let mut modified_files: Vec<_> = repo_status.modified_files.iter().collect();
    modified_files.sort();
    for path in modified_files {
        working.push(status_entry_json(path, "modified", false, None));
    }

    let mut moved_files = repo_status.moved_files.clone();
    moved_files.sort_by(|(a, _, _), (b, _, _)| a.cmp(b));
    for (path, removed_path, _) in moved_files {
        let mut value = status_entry_json(&path, "moved", false, None);
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "from_path".to_string(),
                serde_json::Value::String(path_to_string(&removed_path)),
            );
        }
        working.push(value);
    }

    let mut untracked_files = repo_status.untracked_files.clone();
    untracked_files.sort();
    for path in untracked_files {
        working.push(status_entry_json(&path, "untracked", false, None));
    }

    let mut untracked_dirs = repo_status.untracked_dirs.clone();
    untracked_dirs.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (path, num_files) in untracked_dirs {
        if let Some(repo) = repo {
            let file_paths = untracked_dir_file_paths(repo, &path).await?;
            if file_paths.is_empty() {
                working.push(status_entry_json(&path, "untracked", true, Some(num_files)));
            } else {
                for path in file_paths {
                    working.push(status_entry_json(&path, "untracked", false, None));
                }
            }
        } else {
            working.push(status_entry_json(&path, "untracked", true, Some(num_files)));
        }
    }

    let mut removed_files: Vec<_> = repo_status.removed_files.iter().collect();
    removed_files.sort();
    for path in removed_files {
        working.push(status_entry_json(path, "removed", false, None));
    }

    Ok(serde_json::json!({
        "branch": current_branch.map(|branch| serde_json::json!({
            "name": branch.name,
            "commit_id": branch.commit_id,
        })),
        "head": head.map(|commit| serde_json::json!({
            "commit_id": commit.id,
            "message": commit.message,
        })),
        "is_clean": repo_status.is_clean(),
        "staged": staged,
        "working": working,
    }))
}

async fn untracked_dir_file_paths(
    repo: &LocalRepository,
    dir_path: &Path,
) -> Result<Vec<PathBuf>, OxenError> {
    let gitignore = oxenignore::create(repo);
    let mut dirs = vec![dir_path.to_path_buf()];
    let mut files = Vec::new();

    while let Some(relative_dir) = dirs.pop() {
        let full_dir = repo.path.join(&relative_dir);
        let mut entries = tokio::fs::read_dir(&full_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            let relative_path = relative_dir.join(entry.file_name());
            let is_dir = file_type.is_dir();
            if oxenignore::is_ignored(&relative_path, &gitignore, is_dir) {
                continue;
            }
            if is_dir {
                dirs.push(relative_path);
            } else if file_type.is_file() || file_type.is_symlink() {
                files.push(relative_path);
            }
        }
    }

    files.sort();
    Ok(files)
}

fn status_entry_json(
    path: &Path,
    status: &str,
    is_directory: bool,
    num_files: Option<usize>,
) -> serde_json::Value {
    serde_json::json!({
        "path": path_to_string(path),
        "status": status,
        "is_directory": is_directory,
        "num_files": num_files,
    })
}

fn staged_entry_status_str(status: &StagedEntryStatus) -> &'static str {
    match status {
        StagedEntryStatus::Added => "added",
        StagedEntryStatus::Modified => "modified",
        StagedEntryStatus::Removed => "removed",
        StagedEntryStatus::Unmodified => "unmodified",
    }
}

fn path_to_string(path: &Path) -> String {
    path.to_str()
        .expect("Oxen status paths must be valid UTF-8")
        .to_string()
}

fn parse_ignore_files(paths: Option<&String>) -> Option<HashSet<PathBuf>> {
    let paths_str = paths?;

    match glob(paths_str) {
        Ok(paths) => {
            let mut results: HashSet<PathBuf> = HashSet::new();
            for path in paths.flatten() {
                results.insert(path);
            }
            Some(results)
        }
        Err(err) => {
            log::error!("Err: {err:?}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liboxen::model::{StagedData, StagedEntry, StagedEntryStatus};
    use liboxen::{repositories, util};
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[tokio::test]
    async fn status_json_groups_staged_and_working_entries() -> Result<(), OxenError> {
        let mut status = StagedData::empty();
        status.staged_files.insert(
            PathBuf::from("tracked.yml"),
            StagedEntry::empty_status(StagedEntryStatus::Modified),
        );
        status.modified_files.insert(PathBuf::from("working.yml"));
        status.untracked_files.push(PathBuf::from("new.json"));
        status.untracked_dirs.push((PathBuf::from("assets"), 3));
        status.removed_files.insert(PathBuf::from("old.har"));

        let json = status_json_value(&status, None, None, None).await?;

        assert_eq!(
            json,
            serde_json::json!({
                "branch": null,
                "head": null,
                "is_clean": false,
                "staged": [
                    {
                        "path": "tracked.yml",
                        "status": "modified",
                        "is_directory": false,
                        "num_files": null
                    }
                ],
                "working": [
                    {
                        "path": "working.yml",
                        "status": "modified",
                        "is_directory": false,
                        "num_files": null
                    },
                    {
                        "path": "new.json",
                        "status": "untracked",
                        "is_directory": false,
                        "num_files": null
                    },
                    {
                        "path": "assets",
                        "status": "untracked",
                        "is_directory": true,
                        "num_files": 3
                    },
                    {
                        "path": "old.har",
                        "status": "removed",
                        "is_directory": false,
                        "num_files": null
                    }
                ]
            })
        );

        Ok(())
    }

    #[tokio::test]
    async fn status_json_expands_untracked_dirs_when_repo_available() -> Result<(), OxenError> {
        let temp_dir = TempDir::new()?;
        let repo = repositories::init(temp_dir.path())?;
        util::fs::write_to_path(repo.path.join("episode/yml/step_000.yml"), "step: 0\n")?;
        util::fs::write_to_path(repo.path.join("episode/yml/step_001.yml"), "step: 1\n")?;

        let mut status = StagedData::empty();
        status.untracked_dirs.push((PathBuf::from("episode"), 2));

        let json = status_json_value(&status, None, None, Some(&repo)).await?;

        assert_eq!(
            json["working"],
            serde_json::json!([
                {
                    "path": "episode/yml/step_000.yml",
                    "status": "untracked",
                    "is_directory": false,
                    "num_files": null
                },
                {
                    "path": "episode/yml/step_001.yml",
                    "status": "untracked",
                    "is_directory": false,
                    "num_files": null
                }
            ])
        );

        Ok(())
    }

    #[tokio::test]
    async fn parse_status_paths_skips_unmatched_inputs() -> Result<(), OxenError> {
        let temp_dir = TempDir::new()?;
        let repo = repositories::init(temp_dir.path())?;

        let tracked_path = PathBuf::from("nested/site.yml");
        util::fs::write_to_path(repo.path.join(&tracked_path), "hello\n")?;

        // Existing path should be kept.
        let parsed_existing =
            parse_status_paths(&repo, std::slice::from_ref(&tracked_path)).await?;
        assert_eq!(parsed_existing, vec![repo.path.join(&tracked_path)]);

        // Wildcard that matches nothing should produce no paths so `oxen status` only
        // evaluates an empty scope, matching git-style behavior.
        let parsed_missing_wildcard =
            parse_status_paths(&repo, &[PathBuf::from("*/missing-site.yml")]).await?;
        assert!(parsed_missing_wildcard.is_empty());

        // Non-wildcard path that does not exist should also be skipped, unless tracked.
        let parsed_missing_literal =
            parse_status_paths(&repo, &[PathBuf::from("does_not_exist.yml")]).await?;
        assert!(parsed_missing_literal.is_empty());

        let opts = StagedDataOpts {
            paths: parsed_missing_literal,
            ..StagedDataOpts::default()
        };
        let repo_status = repositories::status::status_from_opts(&repo, &opts).await?;
        assert!(repo_status.is_clean());

        Ok(())
    }
}
