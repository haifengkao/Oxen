use crate::cmd::RunCmd;
use crate::helpers::check_repo_migration_needed;
use async_trait::async_trait;
use clap::{Arg, ArgMatches, Command};
use liboxen::error::OxenError;
use liboxen::model::LocalRepository;
use liboxen::repositories;

pub const NAME: &str = "offline";
pub struct OfflineCmd;

#[async_trait]
impl RunCmd for OfflineCmd {
    fn name(&self) -> &str {
        NAME
    }

    fn args(&self) -> Command {
        Command::new(NAME)
            .about("Inspect files whose working-tree content has been dropped offline.")
            .subcommand(
                Command::new("list").about("List offline files").arg(
                    Arg::new("json")
                        .long("json")
                        .help("Print offline files as machine-readable JSON.")
                        .action(clap::ArgAction::SetTrue),
                ),
            )
    }

    async fn run(&self, args: &ArgMatches) -> Result<(), OxenError> {
        let repo = LocalRepository::from_current_dir()?;
        check_repo_migration_needed(&repo)?;

        let json = args
            .subcommand_matches("list")
            .map(|matches| matches.get_flag("json"))
            .unwrap_or(false);
        let entries = repositories::offline::list(&repo)?;

        if json {
            println!("{}", serde_json::to_string(&entries)?);
            return Ok(());
        }

        for entry in entries {
            println!(
                "{} {} {} bytes",
                entry.path.display(),
                entry.hash,
                entry.num_bytes
            );
        }

        Ok(())
    }
}
