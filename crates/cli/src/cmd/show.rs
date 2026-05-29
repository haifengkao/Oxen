use async_trait::async_trait;
use clap::{Arg, ArgMatches, Command};
use liboxen::error::OxenError;
use liboxen::model::{Commit, LocalRepository};
use liboxen::repositories;
use std::path::PathBuf;
use time::format_description::well_known::Rfc3339;

use crate::cmd::RunCmd;

pub const NAME: &str = "show";
pub struct ShowCmd;

#[derive(Clone, Debug)]
struct ChangedFile {
    path: String,
    status: String,
    is_directory: bool,
}

#[async_trait]
impl RunCmd for ShowCmd {
    fn name(&self) -> &str {
        NAME
    }

    fn args(&self) -> Command {
        Command::new(NAME)
            .about("Show commit details.")
            .arg(Arg::new("commit").required(true))
            .arg(
                Arg::new("json")
                    .long("json")
                    .help("Print machine-readable JSON.")
                    .action(clap::ArgAction::SetTrue),
            )
    }

    async fn run(&self, args: &ArgMatches) -> Result<(), anyhow::Error> {
        let repo = LocalRepository::from_current_dir()?;
        let revision = args
            .get_one::<String>("commit")
            .expect("Must supply commit");
        let commit = repositories::revisions::get(&repo, revision)?
            .ok_or_else(|| OxenError::RevisionNotFound(revision.as_str().into()))?;
        let changed_files = changed_files_for_commit(&repo, &commit).await?;

        if args.get_flag("json") {
            println!(
                "{}",
                serde_json::to_string(&show_json_value(&commit, changed_files))?
            );
            return Ok(());
        }

        println!("commit {}", commit.id);
        println!("Author: {}", commit.author);
        println!("Date:   {}", commit.timestamp.format(&Rfc3339).unwrap());
        println!();
        println!("    {}", commit.message);
        Ok(())
    }
}

async fn changed_files_for_commit(
    repo: &LocalRepository,
    commit: &Commit,
) -> Result<Vec<ChangedFile>, OxenError> {
    let Some(parent_id) = commit.parent_ids.first() else {
        return Ok(vec![]);
    };
    let parent = repositories::commits::get_by_id(repo, parent_id)?
        .ok_or_else(|| OxenError::commit_id_does_not_exist(parent_id))?;
    let entries = repositories::diffs::list_diff_entries(
        repo,
        &parent,
        commit,
        PathBuf::from(""),
        PathBuf::from(""),
        0,
        1_000_000,
    )
    .await?;

    Ok(entries
        .entries
        .into_iter()
        .map(|entry| ChangedFile {
            path: entry.filename,
            status: entry.status,
            is_directory: entry.is_dir,
        })
        .collect())
}

fn show_json_value(commit: &Commit, changed_files: Vec<ChangedFile>) -> serde_json::Value {
    let changed_files: Vec<serde_json::Value> = changed_files
        .into_iter()
        .map(|file| {
            serde_json::json!({
                "path": file.path,
                "status": file.status,
                "is_directory": file.is_directory,
            })
        })
        .collect();

    serde_json::json!({
        "commit": {
            "commit_id": commit.id,
            "parent_ids": commit.parent_ids,
            "message": commit.message,
            "author": commit.author,
            "email": commit.email,
            "timestamp": commit.timestamp.format(&Rfc3339).expect("commit timestamp must format as RFC3339"),
        },
        "changed_files": changed_files,
    })
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    #[test]
    fn show_json_includes_commit_and_changed_files() {
        let commit = liboxen::model::Commit {
            id: "abc123".to_string(),
            parent_ids: vec!["parent123".to_string()],
            message: "Add data".to_string(),
            author: "Ox".to_string(),
            email: "ox@example.com".to_string(),
            timestamp: OffsetDateTime::from_unix_timestamp(0).unwrap(),
        };

        let json = super::show_json_value(
            &commit,
            vec![super::ChangedFile {
                path: "data.yml".to_string(),
                status: "modified".to_string(),
                is_directory: false,
            }],
        );

        assert_eq!(
            json,
            serde_json::json!({
                "commit": {
                    "commit_id": "abc123",
                    "parent_ids": ["parent123"],
                    "message": "Add data",
                    "author": "Ox",
                    "email": "ox@example.com",
                    "timestamp": "1970-01-01T00:00:00Z"
                },
                "changed_files": [
                    {
                        "path": "data.yml",
                        "status": "modified",
                        "is_directory": false
                    }
                ]
            })
        );
    }
}
