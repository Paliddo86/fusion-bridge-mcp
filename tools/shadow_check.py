"""Ship gate: catch local imports that shadow a module name USED EARLIER in the function.

The 1.9.128 regression in one sentence: a guard at the top of _orchestrate_open_lbr used
`os.path.exists`, a `import os` 150 lines later made `os` local to the whole scope, and
every open crashed with UnboundLocalError - reported as the caller's file being corrupt.
The matrix lost a full run to it within the hour (issue #47).

Run over any file(s); exits 1 with findings, 0 clean. Wired next to the AST parse in the
release ritual - same class of static check, same moment.
"""
import ast
import sys


def check(path: str) -> list:
    tree = ast.parse(open(path, encoding="utf-8").read())
    findings = []
    for fn in ast.walk(tree):
        if not isinstance(fn, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        # names bound by LOCAL imports inside this function (not nested functions)
        local_imports = {}   # name -> lineno of the import
        for node in ast.walk(fn):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node is not fn:
                continue
            if isinstance(node, ast.Import):
                for a in node.names:
                    k = a.asname or a.name.split(".")[0]
                    local_imports[k] = min(local_imports.get(k, node.lineno), node.lineno)
            elif isinstance(node, ast.ImportFrom):
                for a in node.names:
                    k = a.asname or a.name
                    local_imports[k] = min(local_imports.get(k, node.lineno), node.lineno)
        if not local_imports:
            continue
        # any LOAD of one of those names BEFORE its import line = UnboundLocalError at runtime
        for node in ast.walk(fn):
            if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Load):
                imp_line = local_imports.get(node.id)
                if imp_line and node.lineno < imp_line:
                    findings.append(
                        "%s:%d: `%s` used in %s() before its local import on line %d "
                        "- the local import makes it local to the WHOLE function, so "
                        "this line raises UnboundLocalError" % (
                            path, node.lineno, node.id, fn.name, imp_line))
    return findings


if __name__ == "__main__":
    all_findings = []
    for p in sys.argv[1:] or ["server.py"]:
        all_findings += check(p)
    for f in all_findings:
        print(f)
    print("%d shadowing finding(s)" % len(all_findings))
    sys.exit(1 if all_findings else 0)
