use async_trait::async_trait;
use clap::{Arg, ArgMatches, Command};
use colored::Colorize;
use minus::Pager;
use std::fmt::Write;
use time::format_description;
use time::format_description::well_known::Rfc3339;

use liboxen::error::OxenError;
use liboxen::model::{Commit, LocalRepository};
use liboxen::repositories;

use crate::cmd::RunCmd;
pub const NAME: &str = "log";
pub struct LogCmd;

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
    }

    async fn run(&self, args: &ArgMatches) -> Result<(), OxenError> {
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
        let revision = args.get_one::<String>("revision").map(String::from);
        self.log_commits(
            &repo,
            revision,
            skip,
            num_commits,
            args.get_flag("json"),
            args.get_flag("no_count_cache"),
        )
        .await?;

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
    pub async fn log_commits(
        &self,
        repo: &LocalRepository,
        revision: Option<String>,
        skip: usize,
        num_commits: usize,
        as_json: bool,
        no_count_cache: bool,
    ) -> Result<(), OxenError> {
        let revision = match revision {
            Some(revision) => revision,
            None => repositories::commits::head_commit(repo)?.id,
        };
        let commits = if no_count_cache {
            repositories::commits::list_from_without_count_cache(
                repo,
                &revision,
                skip,
                num_commits,
            )?
        } else {
            repositories::commits::list_from(repo, &revision)?
                .into_iter()
                .skip(skip)
                .take(num_commits)
                .collect()
        };

        if as_json {
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
                .log_commits(&repo, Some("main".to_string()), 0, 2, true, true)
                .await?;

            assert!(!commit_count_dir.exists());

            Ok(())
        })
        .await
    }
}
