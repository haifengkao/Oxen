use async_trait::async_trait;
use clap::{Arg, ArgMatches, Command};
use liboxen::model::LocalRepository;
use std::path::Path;

use crate::cmd::RunCmd;

pub const NAME: &str = "root";
pub struct RootCmd;

#[async_trait]
impl RunCmd for RootCmd {
    fn name(&self) -> &str {
        NAME
    }

    fn args(&self) -> Command {
        Command::new(NAME)
            .about("Print the repository root path.")
            .arg(
                Arg::new("json")
                    .long("json")
                    .help("Print machine-readable JSON.")
                    .action(clap::ArgAction::SetTrue),
            )
    }

    async fn run(&self, args: &ArgMatches) -> Result<(), anyhow::Error> {
        let repo = LocalRepository::from_current_dir()?;
        if args.get_flag("json") {
            println!("{}", serde_json::to_string(&root_json_value(&repo.path))?);
        } else {
            println!("{}", path_to_string(&repo.path));
        }
        Ok(())
    }
}

fn root_json_value(path: &Path) -> serde_json::Value {
    serde_json::json!({
        "path": path_to_string(path)
    })
}

fn path_to_string(path: &Path) -> String {
    path.to_str()
        .expect("Oxen repository root path must be valid UTF-8")
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn root_json_returns_repo_path() {
        let json = super::root_json_value(&PathBuf::from("/tmp/repo"));

        assert_eq!(
            json,
            serde_json::json!({
                "path": "/tmp/repo"
            })
        );
    }
}
