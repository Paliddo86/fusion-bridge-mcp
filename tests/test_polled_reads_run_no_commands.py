"""Issue #131: the reads the bridge polls must never start a Fusion command (a text command in an
electronics editor ends the user's command in progress)."""
import ast
from pathlib import Path

ADDIN = Path(__file__).resolve().parents[1] / "addin" / "AdomBridge" / "commands"
POLLED = {"app_state.py": "handle_get_app_state", "document_info.py": "handle_document_info", "ui_state.py": None}


def calls_in(node):
    return {n.func.attr for n in ast.walk(node) if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute)}


def test_polled_reads_never_run_a_text_command():
    for name, fn in POLLED.items():
        tree = ast.parse((ADDIN / name).read_text(encoding="utf-8"))
        scope = tree if fn is None else next(n for n in ast.walk(tree) if isinstance(n, ast.FunctionDef) and n.name == fn)
        assert "executeTextCommand" not in calls_in(scope), f"{name}:{fn or 'module'} runs a text command"


if __name__ == "__main__":
    test_polled_reads_never_run_a_text_command()
    print("ok")
