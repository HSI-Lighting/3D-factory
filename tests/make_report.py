#!/usr/bin/env python3
"""Turn a pytest junit XML into tests/report/report.md + failures.txt.

Usage: python3 tests/make_report.py [test-results.xml]
"""

import sys
import xml.etree.ElementTree as ET
from collections import Counter, defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
OUT_DIR = REPO / "tests" / "report"


def main():
    xml_path = Path(sys.argv[1]) if len(sys.argv) > 1 \
        else REPO / "test-results.xml"
    tree = ET.parse(xml_path)
    root = tree.getroot()

    suites = root.findall("testsuite")
    total = sum(int(s.get("tests", 0)) for s in suites)
    failures = sum(int(s.get("failures", 0)) for s in suites)
    errors = sum(int(s.get("errors", 0)) for s in suites)
    skipped = sum(int(s.get("skipped", 0)) for s in suites)
    passed = total - failures - errors - skipped

    per_module = Counter()
    module_fails = defaultdict(list)
    for suite in suites:
        for case in suite.findall("testcase"):
            mod = case.get("classname", suite.get("name", "?"))
            mod = mod.split(".")[-1] if "." in mod else mod
            per_module[mod] += 1
            for kind in ("failure", "error"):
                node = case.find(kind)
                if node is None:
                    continue
                module_fails[mod].append((case.get("name", "?"),
                                          node.get("message", "")
                                          or (node.text or "")[:2000]))

    OUT_DIR.mkdir(parents=True, exist_ok=True)

    lines = ["# cad_cli Python Test Suite — Report",
             "",
             f"Run: {root.get('name', 'pytest')}",
             "",
             f"- **total:** {total}",
             f"- **passed:** {passed}",
             f"- **failed:** {failures + errors}",
             f"- **skipped:** {skipped}",
             "",
             "## Per-module",
             "",
             "| module | tests | failed |",
             "|--------|-------|--------|"]
    for mod, n in sorted(per_module.items()):
        lines.append(f"| {mod} | {n} | {len(module_fails.get(mod, []))} |")

    lines.append("")
    if failures + errors:
        lines.append("## Failing cases")
        lines.append("")
        for mod in sorted(module_fails):
            lines.append(f"### {mod}")
            lines.append("")
            for name, msg in module_fails[mod]:
                lines.append(f"- `{name}`")
                lines.append("")
                lines.append("```")
                lines.append((msg or "no message")[:3000])
                lines.append("```")
                lines.append("")
    else:
        lines.append("No failures.")

    report_md = OUT_DIR / "report.md"
    report_md.write_text("\n".join(lines), encoding="utf-8")

    with (OUT_DIR / "failures.txt").open("w", encoding="utf-8") as f:
        if not (failures + errors):
            f.write("no failures\n")
        for mod in sorted(module_fails):
            for name, msg in module_fails[mod]:
                f.write(f"=== {mod} :: {name}\n")
                f.write(f"{msg}\n\n")

    print(f"report written: {report_md}")
    print(f"failures: {failures + errors} / total: {total}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
