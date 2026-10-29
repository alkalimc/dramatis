"""Where the artifact store goes."""

from __future__ import annotations

from dramatis_forge import config


def test_fresh_clone_uses_a_sibling_of_the_repository(tmp_path):
    repo = tmp_path / "work" / "dramatis"
    (repo / ".git").mkdir(parents=True)
    (repo / "forge").mkdir()
    (repo / "forge" / "pyproject.toml").write_text("", encoding="utf-8")
    for start in (repo, repo / "forge"):
        assert config.workspace_root(start) == (tmp_path / "work").resolve()


def test_git_file_of_a_worktree_counts(tmp_path):
    repo = tmp_path / "wt" / "one"
    repo.mkdir(parents=True)
    (repo / ".git").write_text("gitdir: elsewhere", encoding="utf-8")
    assert config.workspace_root(repo) == (tmp_path / "wt").resolve()


def test_existing_artifacts_directory_wins(tmp_path):
    (tmp_path / config.ARTIFACTS_DIR).mkdir()
    repo = tmp_path / "a" / "b" / "dramatis"
    (repo / ".git").mkdir(parents=True)
    assert config.workspace_root(repo) == tmp_path.resolve()


def test_no_repository_uses_the_start_directory(tmp_path):
    start = tmp_path / "scratch"
    start.mkdir()
    assert config.workspace_root(start) == start.resolve()


def test_env_overrides_the_default(tmp_path, monkeypatch):
    monkeypatch.setenv(config.ENV_HOME, str(tmp_path / "elsewhere"))
    assert config.forge_home() == (tmp_path / "elsewhere").resolve()


def test_package_version_matches_pyproject():
    import tomllib
    from pathlib import Path

    from dramatis_forge import __version__

    pyproject = Path(__file__).resolve().parents[1] / "pyproject.toml"
    declared = tomllib.loads(pyproject.read_text(encoding="utf-8"))["project"]["version"]
    assert __version__ == declared
