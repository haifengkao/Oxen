//! Offline working-tree content management.
//!
//! This keeps tracked file metadata in Oxen, while allowing clean working-tree
//! files to be removed locally and restored later from the configured version
//! store.

use crate::constants::OXEN_HIDDEN_DIR;
use crate::core::v_latest::index::restore as index_restore;
use crate::error::OxenError;
use crate::model::merkle_tree::node::{EMerkleTreeNode, FileNode, MerkleTreeNode};
use crate::model::{Commit, LocalRepository};
use crate::{repositories, util};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const OFFLINE_DIR: &str = "offline";
const OFFLINE_INDEX_FILE: &str = "files.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OfflineEntry {
    pub path: PathBuf,
    pub hash: String,
    pub commit_id: String,
    pub num_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct OfflineIndex {
    files: BTreeMap<String, OfflineEntry>,
}

impl OfflineIndex {
    pub(crate) fn is_current(&self, path: &Path, hash: &str) -> Result<bool, OxenError> {
        let key = path_key(path)?;
        Ok(self
            .files
            .get(&key)
            .map(|entry| entry.hash == hash)
            .unwrap_or(false))
    }
}

pub(crate) fn load_index(repo: &LocalRepository) -> Result<OfflineIndex, OxenError> {
    let path = index_path(repo);
    if !path.exists() {
        return Ok(OfflineIndex::default());
    }

    let data = util::fs::read_from_path(&path)?;
    if data.trim().is_empty() {
        return Ok(OfflineIndex::default());
    }

    serde_json::from_str(&data).map_err(|err| {
        OxenError::basic_str(format!(
            "Could not read offline index {}: {err}",
            path.display()
        ))
    })
}

pub fn list(repo: &LocalRepository) -> Result<Vec<OfflineEntry>, OxenError> {
    Ok(load_index(repo)?.files.into_values().collect())
}

pub async fn drop_paths(
    repo: &LocalRepository,
    paths: &[PathBuf],
) -> Result<Vec<OfflineEntry>, OxenError> {
    let commit = repositories::commits::head_commit(repo)?;
    let version_store = repo.version_store()?;
    let mut index = load_index(repo)?;
    let mut dropped = Vec::new();

    for input_path in paths {
        let relative_path = normalize_repo_path(repo, input_path)?;
        let files = collect_head_files(repo, &commit, &relative_path)?;

        for (file_path, file_node) in files {
            let hash = file_node.hash().to_string();
            if !version_store.version_exists(&hash).await? {
                return Err(OxenError::basic_str(format!(
                    "Cannot drop {} because version {} is not available in the configured version store",
                    file_path.display(),
                    hash
                )));
            }

            let working_path = repo.path.join(&file_path);
            if working_path.exists() {
                if util::fs::is_modified_from_node_with_metadata(
                    &working_path,
                    &file_node,
                    util::fs::metadata(&working_path),
                )? {
                    return Err(OxenError::basic_str(format!(
                        "Refusing to drop modified file {}",
                        file_path.display()
                    )));
                }
            } else if !index.is_current(&file_path, &hash)? {
                return Err(OxenError::basic_str(format!(
                    "Cannot drop missing non-offline file {}",
                    file_path.display()
                )));
            }

            let entry = OfflineEntry {
                path: file_path.clone(),
                hash,
                commit_id: commit.id.clone(),
                num_bytes: file_node.num_bytes(),
            };
            let key = path_key(&file_path)?;
            let previous = index.files.insert(key.clone(), entry.clone());
            save_index(repo, &index)?;

            if working_path.exists()
                && let Err(err) = util::fs::remove_file(&working_path)
            {
                if let Some(previous) = previous {
                    index.files.insert(key, previous);
                } else {
                    index.files.remove(&key);
                }
                save_index(repo, &index)?;
                return Err(err);
            }

            dropped.push(entry);
        }
    }

    Ok(dropped)
}

pub async fn get_paths(
    repo: &LocalRepository,
    paths: &[PathBuf],
) -> Result<Vec<OfflineEntry>, OxenError> {
    let commit = repositories::commits::head_commit(repo)?;
    let version_store = repo.version_store()?;
    let mut index = load_index(repo)?;
    let mut restored = Vec::new();

    for input_path in paths {
        let relative_path = normalize_repo_path(repo, input_path)?;
        let files = collect_head_files(repo, &commit, &relative_path)?;

        for (file_path, file_node) in files {
            let working_path = repo.path.join(&file_path);
            if working_path.exists()
                && util::fs::is_modified_from_node_with_metadata(
                    &working_path,
                    &file_node,
                    util::fs::metadata(&working_path),
                )?
            {
                return Err(OxenError::basic_str(format!(
                    "Refusing to overwrite modified file {}",
                    file_path.display()
                )));
            }

            index_restore::restore_file(repo, &file_node, &file_path, &version_store).await?;

            let key = path_key(&file_path)?;
            let entry = index.files.remove(&key).unwrap_or_else(|| OfflineEntry {
                path: file_path.clone(),
                hash: file_node.hash().to_string(),
                commit_id: commit.id.clone(),
                num_bytes: file_node.num_bytes(),
            });
            restored.push(entry);
        }
    }

    save_index(repo, &index)?;
    Ok(restored)
}

fn save_index(repo: &LocalRepository, index: &OfflineIndex) -> Result<(), OxenError> {
    let path = index_path(repo);
    if let Some(parent) = path.parent() {
        util::fs::create_dir_all(parent)?;
    }

    let tmp_path = path.with_extension("json.tmp");
    let data = serde_json::to_string_pretty(index)?;
    util::fs::write(&tmp_path, data)?;
    std::fs::rename(&tmp_path, &path).map_err(|err| OxenError::file_error(&path, err))?;
    Ok(())
}

fn index_path(repo: &LocalRepository) -> PathBuf {
    repo.path
        .join(OXEN_HIDDEN_DIR)
        .join(OFFLINE_DIR)
        .join(OFFLINE_INDEX_FILE)
}

fn normalize_repo_path(
    repo: &LocalRepository,
    path: impl AsRef<Path>,
) -> Result<PathBuf, OxenError> {
    let path = path.as_ref();
    let path = if path.is_absolute() {
        util::fs::path_relative_to_dir(path, &repo.path)?
    } else {
        util::fs::path_relative_to_dir(repo.path.join(path), &repo.path)?
    };
    Ok(path)
}

fn path_key(path: &Path) -> Result<String, OxenError> {
    path.to_str()
        .map(String::from)
        .ok_or_else(|| OxenError::basic_str(format!("Offline path is not valid UTF-8: {path:?}")))
}

fn collect_head_files(
    repo: &LocalRepository,
    commit: &Commit,
    path: &Path,
) -> Result<Vec<(PathBuf, FileNode)>, OxenError> {
    if let Some(file_node) = repositories::tree::get_file_by_path(repo, commit, path)? {
        return Ok(vec![(path.to_path_buf(), file_node)]);
    }

    let Some(dir_node) =
        repositories::tree::get_dir_with_children_recursive(repo, commit, path, None)?
    else {
        return Err(OxenError::entry_does_not_exist_in_commit(path, &commit.id));
    };

    let mut entries = Vec::new();
    collect_files_from_node(&dir_node, path, &mut entries)?;
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));
    Ok(entries)
}

fn collect_files_from_node(
    node: &MerkleTreeNode,
    base_path: &Path,
    entries: &mut Vec<(PathBuf, FileNode)>,
) -> Result<(), OxenError> {
    match &node.node {
        EMerkleTreeNode::File(file_node) => {
            entries.push((base_path.to_path_buf(), file_node.clone()));
        }
        EMerkleTreeNode::Directory(_) | EMerkleTreeNode::VNode(_) | EMerkleTreeNode::Commit(_) => {
            for child in &node.children {
                match &child.node {
                    EMerkleTreeNode::File(file_node) => {
                        entries.push((base_path.join(file_node.name()), file_node.clone()));
                    }
                    EMerkleTreeNode::Directory(dir_node) => {
                        collect_files_from_node(child, &base_path.join(dir_node.name()), entries)?;
                    }
                    EMerkleTreeNode::VNode(_) | EMerkleTreeNode::Commit(_) => {
                        collect_files_from_node(child, base_path, entries)?;
                    }
                    _ => {}
                }
            }
        }
        _ => {
            return Err(OxenError::basic_str(format!(
                "Unexpected node type in offline traversal: {:?}",
                node.node.node_type()
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{repositories, test, util};
    use std::path::PathBuf;

    #[tokio::test]
    async fn drop_get_roundtrip_keeps_status_clean() -> Result<(), OxenError> {
        test::run_empty_local_repo_test_async(|repo| async move {
            let relative_path = PathBuf::from("hello.txt");
            let file_path = repo.path.join(&relative_path);
            util::fs::write_to_path(&file_path, "hello offline")?;
            repositories::add(&repo, &file_path).await?;
            repositories::commit(&repo, "add hello")?;

            let dropped = drop_paths(&repo, std::slice::from_ref(&relative_path)).await?;
            assert_eq!(dropped.len(), 1);
            assert!(!file_path.exists());
            assert!(repositories::status(&repo)?.is_clean());
            assert_eq!(list(&repo)?.len(), 1);

            let restored = get_paths(&repo, std::slice::from_ref(&relative_path)).await?;
            assert_eq!(restored.len(), 1);
            assert_eq!(util::fs::read_from_path(&file_path)?, "hello offline");
            assert!(repositories::status(&repo)?.is_clean());
            assert!(list(&repo)?.is_empty());

            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn drop_refuses_modified_files() -> Result<(), OxenError> {
        test::run_empty_local_repo_test_async(|repo| async move {
            let relative_path = PathBuf::from("hello.txt");
            let file_path = repo.path.join(&relative_path);
            util::fs::write_to_path(&file_path, "committed")?;
            repositories::add(&repo, &file_path).await?;
            repositories::commit(&repo, "add hello")?;

            util::fs::write_to_path(&file_path, "modified")?;
            let err = drop_paths(&repo, std::slice::from_ref(&relative_path))
                .await
                .unwrap_err();

            assert!(format!("{err}").contains("Refusing to drop modified file"));
            assert!(file_path.exists());
            assert!(list(&repo)?.is_empty());

            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn drop_directory_marks_all_tracked_files_offline() -> Result<(), OxenError> {
        test::run_empty_local_repo_test_async(|repo| async move {
            let dir = PathBuf::from("data");
            let first = repo.path.join("data/one.txt");
            let second = repo.path.join("data/nested/two.txt");
            util::fs::write_to_path(&first, "one")?;
            util::fs::write_to_path(&second, "two")?;
            repositories::add(&repo, repo.path.join(&dir)).await?;
            repositories::commit(&repo, "add data")?;

            let dropped = drop_paths(&repo, std::slice::from_ref(&dir)).await?;

            assert_eq!(dropped.len(), 2);
            assert!(!first.exists());
            assert!(!second.exists());
            assert!(repositories::status(&repo)?.is_clean());

            Ok(())
        })
        .await
    }
}
