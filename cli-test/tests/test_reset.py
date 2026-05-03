"""
Tests for oxen reset command.
"""

import json
import os

from tests.helpers import create_test_file


class TestResetCommand:
    """Test suite for oxen reset command."""

    def test_reset_hard_discards_all_local_changes(self, test_dir, oxen):
        """Test oxen reset --hard discards staged, tracked, and untracked changes."""

        repo_path = test_dir / "test-reset-hard"
        repo_path.mkdir(parents=True, exist_ok=True)
        os.chdir(repo_path)

        oxen.run("init")

        create_test_file("tracked.txt", "original\n")
        create_test_file("removed.txt", "keep me\n")
        oxen.run("add", "tracked.txt", "removed.txt")
        oxen.run("commit", "-m", "initial commit")

        create_test_file("tracked.txt", "modified\n")
        oxen.run("add", "tracked.txt")
        create_test_file("tracked.txt", "modified again\n")

        os.remove("removed.txt")
        create_test_file("staged_new.txt", "staged\n")
        oxen.run("add", "staged_new.txt")

        create_test_file("untracked.txt", "temporary\n")
        create_test_file("scratch/nested.txt", "temporary\n")

        oxen.run("reset", "--hard")

        assert (repo_path / "tracked.txt").read_text() == "original\n"
        assert (repo_path / "removed.txt").read_text() == "keep me\n"
        assert not (repo_path / "staged_new.txt").exists()
        assert not (repo_path / "untracked.txt").exists()
        assert not (repo_path / "scratch").exists()

        status = oxen.run("status", "--json")
        status_json = json.loads(status.stdout)
        assert status_json["is_clean"] is True

