use async_trait::async_trait;
use clap::{Arg, Command};
use liboxen::core::staged::get_staged_db_manager;
use liboxen::error::OxenError;
use liboxen::model::{LocalRepository, StagedEntryStatus};
use liboxen::{repositories, util};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;

use crate::cmd::RunCmd;

pub const NAME: &str = "cat";
pub struct CatCmd;

fn normalize_repo_path(
    repo: &LocalRepository,
    path: impl AsRef<Path>,
) -> Result<PathBuf, OxenError> {
    let path = path.as_ref();

    if path.is_absolute() {
        return util::fs::path_relative_to_dir(path, &repo.path);
    }

    let current_dir = std::env::current_dir()?;
    util::fs::path_relative_to_dir(current_dir.join(path), &repo.path)
}

async fn read_staged_file(
    repo: &LocalRepository,
    path: impl AsRef<Path>,
) -> Result<Vec<u8>, OxenError> {
    let path = path.as_ref();
    let staged_node = get_staged_db_manager(repo)?
        .read_from_staged_db(path)?
        .ok_or_else(|| OxenError::basic_str(format!("No staged entry found for {path:?}")))?;

    if staged_node.status == StagedEntryStatus::Removed {
        return Err(OxenError::basic_str(format!(
            "Cannot cat staged removed file: {path:?}"
        )));
    }

    let file_node = staged_node.node.file()?;
    let version_store = repo.version_store()?;
    let mut stream = version_store
        .get_version_stream(&file_node.hash().to_string())
        .await?;
    let mut data = Vec::new();
    while let Some(chunk) = stream.next().await {
        data.extend(chunk?);
    }
    Ok(data)
}

#[async_trait]
impl RunCmd for CatCmd {
    fn name(&self) -> &str {
        NAME
    }

    fn args(&self) -> Command {
        Command::new(NAME)
            .about("Print raw file contents from a revision.")
            .arg(Arg::new("path").required(true))
            .arg(
                Arg::new("revision")
                    .long("revision")
                    .short('r')
                    .help("The branch, commit id, or HEAD revision to read from.")
                    .default_value("HEAD")
                    .action(clap::ArgAction::Set),
            )
            .arg(
                Arg::new("staged")
                    .long("staged")
                    .help("Print raw file contents from the staging area.")
                    .action(clap::ArgAction::SetTrue),
            )
    }

    async fn run(&self, args: &clap::ArgMatches) -> Result<(), OxenError> {
        let repository = LocalRepository::from_current_dir()?;
        let path = args.get_one::<String>("path").expect("Must supply path");
        let revision = args
            .get_one::<String>("revision")
            .expect("Must supply revision");
        let repo_path = normalize_repo_path(&repository, PathBuf::from(path))?;
        let mut stdout = tokio::io::stdout();

        if args.get_flag("staged") {
            let data = read_staged_file(&repository, repo_path).await?;
            stdout.write_all(&data).await?;
            stdout.flush().await?;
            return Ok(());
        }

        let mut stream = repositories::revisions::get_version_stream_from_revision(
            &repository,
            revision,
            repo_path,
        )
        .await?;

        while let Some(chunk) = stream.next().await {
            stdout.write_all(&chunk?).await?;
        }
        stdout.flush().await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liboxen::{repositories, test, util};

    #[tokio::test]
    async fn read_staged_file_reads_index_content_without_touching_worktree()
    -> Result<(), OxenError> {
        test::run_empty_local_repo_test_async(|repo| async move {
            let relative_path = PathBuf::from("hello.txt");
            let file_path = repo.path.join(&relative_path);
            util::fs::write_to_path(&file_path, "committed\n")?;
            repositories::add(&repo, &file_path).await?;
            repositories::commit(&repo, "Add hello")?;

            util::fs::write_to_path(&file_path, "staged\n")?;
            repositories::add(&repo, &file_path).await?;
            util::fs::write_to_path(&file_path, "working\n")?;

            let data = read_staged_file(&repo, &relative_path).await?;

            assert_eq!(String::from_utf8(data).unwrap(), "staged\n");
            assert_eq!(util::fs::read_from_path(&file_path)?, "working\n");
            Ok(())
        })
        .await
    }
}
