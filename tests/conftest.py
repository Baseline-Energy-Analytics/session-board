"""Keep the tests independent of the machine's own settings: no config file, no SESSION_BOARD_*
variables, and Claude Code's default folder. Runs before the board's modules are imported."""
import os

for _var in [v for v in os.environ if v.startswith("SESSION_BOARD_")] + ["CLAUDE_CONFIG_DIR"]:
    os.environ.pop(_var, None)
os.environ["SESSION_BOARD_CONFIG"] = os.path.join(os.path.dirname(__file__), "no-such-config.toml")
