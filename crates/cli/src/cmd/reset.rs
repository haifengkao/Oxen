use async_trait::async_trait;
use clap::{Arg, ArgMatches, Command};
use std::collections::HashSet;

use liboxen::error::OxenError;
use liboxen::model::LocalRepository;
use liboxen::opts::{CleanOpts, RestoreOpts};
use liboxen::repositories;

use crate::cmd::RunCmd;
use crate::helpers::check_repo_migration_needed;

pub const NAME: &str = "reset";
pub struct ResetCmd;

#[async_trait]
impl RunCmd for ResetCmd {
    fn name(&self) -> &str {
        NAME
    }

    fn args(&self) -> Command {
        Command::new(NAME)
            .about("Reset local changes")
            .long_about(
                "Reset local changes.\n\n\
                 `oxen reset --hard` discards all staged and working tree changes, \
                 including untracked files and directories.",
            )
            .arg_required_else_help(true)
            .arg(
                Arg::new("hard")
                    .long("hard")
                    .help("Discard all staged, tracked, and untracked local changes.")
                    .action(clap::ArgAction::SetTrue)
                    .required(true),
            )
    }

    async fn run(&self, args: &ArgMatches) -> Result<(), anyhow::Error> {
        let repository = LocalRepository::from_current_dir()?;
        check_repo_migration_needed(&repository)?;

        if args.get_flag("hard") {
            reset_hard(&repository).await?;
        }

        Ok(())
    }
}

async fn reset_hard(repository: &LocalRepository) -> Result<(), OxenError> {
    let root_paths = HashSet::from([repository.path.clone()]);

    repositories::restore::restore(
        repository,
        RestoreOpts {
            paths: root_paths.clone(),
            staged: true,
            is_remote: false,
            source_ref: None,
        },
    )
    .await?;

    if repositories::commits::head_commit_maybe(repository)?.is_some() {
        repositories::restore::restore(
            repository,
            RestoreOpts {
                paths: root_paths,
                staged: false,
                is_remote: false,
                source_ref: None,
            },
        )
        .await?;
    }

    repositories::clean::clean(
        repository,
        &CleanOpts {
            paths: vec![],
            force: true,
        },
    )
    .await?;

    println!("Discarded all local changes.");

    Ok(())
}
