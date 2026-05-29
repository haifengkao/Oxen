use async_trait::async_trait;
use clap::{Arg, ArgMatches, Command};
use colored::Colorize;
use minus::Pager;
use std::fmt::Write;
use std::path::PathBuf;
use time::format_description;
use time::format_description::well_known::Rfc3339;

use liboxen::error::OxenError;
use liboxen::model::{Commit, LocalRepository};
use liboxen::opts::PaginateOpts;
use liboxen::repositories;

use crate::cmd::RunCmd;

pub struct LogCmd;

const NAME: &str = "log";

struct LogOptions {
    revision: Option<String>,
    skip: usize,
    num_commits: usize,
    as_json: bool,
    no_count_cache: bool,
    path: Option<PathBuf>,
}

fn write_to_pager(output: &mut Pager, text: &str) -> Result<(), OxenError> {
    writeln!(output, "{text}")
        .map_err(|e| OxenError::basic_str(format!("Could not write to pager: {e}")))
}

#[async_trait]
impl RunCmd for LogCmd {
    fn name(&self) -> &str {
        NAME
    }

    fn args(&self) -> Command {
        Command::new(NAME)
            .about("See log of commits")
            .arg(
                Arg::new("revision")
                    .long("revision")
                    .help("The commit or branch id you want to get history from. Defaults to main.")
                    .action(clap::ArgAction::Set),
            )
            .arg(
                Arg::new("number")
                    .long("number")
                    .short('n')
                    .help("Number of commits to show")
                    .default_value("20"),
            )
            .arg(
                Arg::new("skip")
                    .long("skip")
                    .help("Number of commits to skip before output.")
                    .default_value("0"),
            )
            .arg(
                Arg::new("json")
                    .long("json")
                    .help("Print machine-readable JSON.")
                    .action(clap::ArgAction::SetTrue),
            )
            .arg(
                Arg::new("no_count_cache")
                    .long("no-count-cache")
                    .help("Do not read or write the commit count cache while listing history.")
                    .action(clap::ArgAction::SetTrue),
            )
            .arg(
                Arg::new("pathspec")
                    .help("Limit history to commits that changed this file or directory.")
                    .num_args(0..=1)
                    .value_name("path"),
            )
    }

    async fn run(&self, args: &ArgMatches) -> Result<(), anyhow::Error> {
        // Look up from the current dir for .oxen directory
        let repo = LocalRepository::from_current_dir()?;

        let num_commits = args
            .get_one::<String>("number")
            .expect("Must supply number")
            .parse::<usize>()
            .expect("number must be a valid integer.");
        let skip = args
            .get_one::<String>("skip")
            .expect("Must supply skip")
            .parse::<usize>()
            .expect("skip must be a valid integer.");
        let opts = LogOptions {
            revision: args.get_one::<String>("revision").map(String::from),
            skip,
            num_commits,
            as_json: args.get_flag("json"),
            no_count_cache: args.get_flag("no_count_cache"),
            path: args.get_one::<String>("pathspec").map(PathBuf::from),
        };
        self.log_commits(&repo, opts).await?;

        Ok(())
    }
}

fn log_json_value(commits: &[Commit]) -> serde_json::Value {
    let commits: Vec<serde_json::Value> = commits
        .iter()
        .map(|commit| {
            serde_json::json!({
                "commit_id": commit.id,
                "parent_ids": commit.parent_ids,
                "message": commit.message,
                "author": commit.author,
                "email": commit.email,
                "timestamp": commit.timestamp.format(&Rfc3339).expect("commit timestamp must format as RFC3339"),
            })
        })
        .collect();

    serde_json::json!({
        "commits": commits,
    })
}

impl LogCmd {
    async fn log_commits(&self, repo: &LocalRepository, opts: LogOptions) -> Result<(), OxenError> {
        let commits = self.resolve_commits(repo, &opts)?;

        if opts.as_json {
            println!("{}", serde_json::to_string(&log_json_value(&commits))?);
            return Ok(());
        }

        // Fri, 21 Oct 2022 16:08:39 -0700
        let format = format_description::parse(
            "[weekday], [day] [month repr:long] [year] [hour]:[minute]:[second] [offset_hour sign:mandatory]",
        ).unwrap();

        let mut output = Pager::new();

        for commit in &commits {
            let commit_id_str = format!("commit {}", commit.id).yellow();
            write_to_pager(&mut output, &format!("{commit_id_str}\n"))?;
            write_to_pager(&mut output, &format!("Author: {}", commit.author))?;
            write_to_pager(
                &mut output,
                &format!("Date:   {}\n", commit.timestamp.format(&format).unwrap()),
            )?;
            write_to_pager(&mut output, &format!("    {}\n", commit.message))?;
        }

        match minus::page_all(output) {
            Ok(_) => {}
            Err(e) => {
                eprintln!("Error while paging: {e}");
            }
        }
        Ok(())
    }

    fn resolve_commits(
        &self,
        repo: &LocalRepository,
        opts: &LogOptions,
    ) -> Result<Vec<Commit>, OxenError> {
        let revision = match &opts.revision {
            Some(revision) => revision.clone(),
            None => repositories::commits::head_commit(repo)?.id,
        };

        if let Some(path) = &opts.path {
            let commit = repositories::commits::get_commit_or_head(repo, Some(revision))?;
            let page_size = opts.skip.saturating_add(opts.num_commits).max(1);
            let paginated = repositories::commits::list_by_path_from_paginated(
                repo,
                &commit,
                path,
                PaginateOpts {
                    page_num: 1,
                    page_size,
                },
            )?;
            return Ok(paginated
                .commits
                .into_iter()
                .skip(opts.skip)
                .take(opts.num_commits)
                .collect());
        }

        if opts.no_count_cache {
            return repositories::commits::list_from_without_count_cache(
                repo,
                &revision,
                opts.skip,
                opts.num_commits,
            );
        }

        let page_size = opts.skip.saturating_add(opts.num_commits).max(1);
        let paginated = repositories::commits::list_from_paginated(
            repo,
            &revision,
            PaginateOpts {
                page_num: 1,
                page_size,
            },
        )?;
        Ok(paginated
            .commits
            .into_iter()
            .skip(opts.skip)
            .take(opts.num_commits)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liboxen::constants::COMMIT_COUNT_DIR;
    use liboxen::{test, util};
    use time::OffsetDateTime;

    #[test]
    fn log_json_uses_commit_id_and_rfc3339_timestamps() {
        let commit = liboxen::model::Commit {
            id: "abc123".to_string(),
            parent_ids: vec!["parent123".to_string()],
            message: "Add data".to_string(),
            author: "Ox".to_string(),
            email: "ox@example.com".to_string(),
            timestamp: OffsetDateTime::from_unix_timestamp(0).unwrap(),
        };

        let json = log_json_value(&[commit]);

        assert_eq!(
            json,
            serde_json::json!({
                "commits": [
                    {
                        "commit_id": "abc123",
                        "parent_ids": ["parent123"],
                        "message": "Add data",
                        "author": "Ox",
                        "email": "ox@example.com",
                        "timestamp": "1970-01-01T00:00:00Z"
                    }
                ]
            })
        );
    }

    #[tokio::test]
    async fn log_commits_without_count_cache_does_not_create_commit_count_db()
    -> Result<(), OxenError> {
        test::run_empty_local_repo_test_async(|repo| async move {
            for i in 0..3 {
                let file_path = repo.path.join(format!("file_{i}.txt"));
                test::write_txt_file_to_path(&file_path, format!("Content {i}"))?;

                repositories::add(&repo, &file_path).await?;
                repositories::commit(&repo, &format!("Commit {i}"))?;
            }

            let commit_count_dir = util::fs::oxen_hidden_dir(&repo.path).join(COMMIT_COUNT_DIR);
            assert!(!commit_count_dir.exists());

            LogCmd
                .log_commits(
                    &repo,
                    LogOptions {
                        revision: Some("main".to_string()),
                        skip: 0,
                        num_commits: 2,
                        as_json: true,
                        no_count_cache: true,
                        path: None,
                    },
                )
                .await?;

            assert!(!commit_count_dir.exists());

            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn log_commits_can_filter_history_by_path() -> Result<(), OxenError> {
        test::run_empty_local_repo_test_async(|repo| async move {
            let target_path = repo.path.join("target.txt");
            util::fs::write_to_path(&target_path, "first")?;
            repositories::add(&repo, &target_path).await?;
            let first_target_commit = repositories::commit(&repo, "add target")?;

            let other_path = repo.path.join("other.txt");
            util::fs::write_to_path(&other_path, "other")?;
            repositories::add(&repo, &other_path).await?;
            repositories::commit(&repo, "add other")?;

            util::fs::write_to_path(&target_path, "second")?;
            repositories::add(&repo, &target_path).await?;
            let second_target_commit = repositories::commit(&repo, "modify target")?;

            let commits = LogCmd.resolve_commits(
                &repo,
                &LogOptions {
                    revision: Some("main".to_string()),
                    skip: 0,
                    num_commits: 10,
                    as_json: true,
                    no_count_cache: true,
                    path: Some(PathBuf::from("target.txt")),
                },
            )?;

            assert_eq!(
                commits
                    .iter()
                    .map(|commit| commit.id.as_str())
                    .collect::<Vec<_>>(),
                vec![
                    second_target_commit.id.as_str(),
                    first_target_commit.id.as_str()
                ]
            );

            Ok(())
        })
        .await
    }

    #[test]
    fn log_args_accept_git_style_pathspec_after_separator() {
        let args = LogCmd
            .args()
            .try_get_matches_from(["log", "--json", "--", "target.txt"])
            .unwrap();
        let paths = args
            .get_many::<String>("pathspec")
            .unwrap()
            .map(String::as_str)
            .collect::<Vec<_>>();

        assert_eq!(paths, vec!["target.txt"]);
    }

    #[test]
    fn log_args_accept_git_style_pathspec_without_separator() {
        let args = LogCmd
            .args()
            .try_get_matches_from(["log", "--json", "target.txt"])
            .unwrap();
        let path = args.get_one::<String>("pathspec").unwrap();

        assert_eq!(path, "target.txt");
    }
}
