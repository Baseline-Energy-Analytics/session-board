import tempfile

import pytest

from board import config


def load(tmp_path, env=None, toml=None):
    path = tmp_path / "config.toml"
    if toml is not None:
        path.write_text(toml)
    return config.load({"HOME": "/home/u", "SESSION_BOARD_CONFIG": str(path), **(env or {})})


def test_defaults(tmp_path):
    c = load(tmp_path)
    assert c.workspaces == ("/home/u",)
    assert c.scratch == tempfile.gettempdir()
    assert (c.days, c.poll, c.claude) == (30, 10.0, "claude")
    assert c.claude_dir == "/home/u/.claude"
    assert c.cache_dir == "/home/u/.cache/session-board"
    assert c.state_dir == "/home/u/.local/state/session-board"


def test_the_file_sets_values_and_expands_home(tmp_path):
    c = load(tmp_path, toml='workspaces = ["~", "/opt", "~/work/"]\nscratch = "~/scratch"\ndays = 14\n')
    assert c.workspaces == ("/home/u", "/opt", "/home/u/work")
    assert c.scratch == "/home/u/scratch" and c.days == 14
    assert c.source["workspaces"].endswith("config.toml") and c.source["poll"] == "default"


def test_environment_wins_over_the_file(tmp_path):
    c = load(tmp_path, env={"SESSION_BOARD_WORKSPACES": "/srv:~/code", "SESSION_BOARD_POLL": "2.5",
                            "SESSION_BOARD_CLAUDE": "/usr/local/bin/claude", "CLAUDE_CONFIG_DIR": "~/.claude-work",
                            "XDG_STATE_HOME": "/state"},
             toml='workspaces = ["/opt"]\npoll = 30\n')
    assert c.workspaces == ("/srv", "/home/u/code")
    assert c.poll == 2.5 and c.claude == "/usr/local/bin/claude"
    assert c.claude_dir == "/home/u/.claude-work" and c.state_dir == "/state/session-board"
    assert c.source["workspaces"] == "$SESSION_BOARD_WORKSPACES"


@pytest.mark.parametrize("toml", ['days = 0\n', 'poll = "soon"\n', 'days = [\n'])
def test_a_bad_value_stops_with_a_message(tmp_path, toml):
    with pytest.raises(SystemExit) as e:
        load(tmp_path, toml=toml)
    assert "session-board" in str(e.value)


def test_describe_names_each_source(tmp_path):
    text = config.describe(load(tmp_path, env={"SESSION_BOARD_DAYS": "7"}))
    assert "not found; defaults apply" in text
    assert "days        7    [$SESSION_BOARD_DAYS]" in text
