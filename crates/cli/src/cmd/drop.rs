use crate::cmd::RunCmd;
use crate::helpers::check_repo_migration_needed;
use async_trait::async_trait;
use clap::{Arg, ArgMatches, Command};
use liboxen::error::OxenError;
use liboxen::model::LocalRepository;
use liboxen::{repositories, util};
use std::path::{Path, PathBuf};

pub const NAME: &str = "drop";
pub struct DropCmd;

#[async_trait]
impl RunCmd for DropCmd {
    fn name(&self) -> &str {
        NAME
    }

    fn args(&self) -> Command {
        Command::new(NAME)
            .about("Remove clean tracked file content from the working tree and mark it offline.")
            .arg(
                Arg::new("paths")
                    .required(true)
                    .action(clap::ArgAction::Append),
            )
            .arg_required_else_help(true)
    }

    async fn run(&self, args: &ArgMatches) -> Result<(), OxenError> {
        let repo = LocalRepository::from_current_dir()?;
        check_repo_migration_needed(&repo)?;
        let paths = collect_paths(&repo, args)?;
        let dropped = repositories::offline::drop_paths(&repo, &paths).await?;
        println!("Dropped {} file(s)", dropped.len());
        Ok(())
    }
}

fn collect_paths(repo: &LocalRepository, args: &ArgMatches) -> Result<Vec<PathBuf>, OxenError> {
    args.get_many::<String>("paths")
        .expect("Must supply paths")
        .map(|p| normalize_path(repo, Path::new(p)))
        .collect()
}

fn normalize_path(repo: &LocalRepository, path: &Path) -> Result<PathBuf, OxenError> {
    if path.is_absolute() {
        return util::fs::path_relative_to_dir(path, &repo.path);
    }

    let current_dir = std::env::current_dir()?;
    util::fs::path_relative_to_dir(current_dir.join(path), &repo.path)
}
