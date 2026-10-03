"""Run a dependency audit, retaining JSON and failing closed on tool errors."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess


def require(condition, message):
    if not condition:
        raise ValueError(message)


def classify(kind, data, status):
    require(type(status) is int and status in (0, 1), "audit execution failed")
    require(isinstance(data, dict) and "error" not in data, "audit error response")
    if kind == "npm":
        require(data.get("auditReportVersion") == 2, "unexpected npm audit schema")
        metadata = data.get("metadata", {})
        dependencies = metadata.get("dependencies", {})
        counts = metadata.get("vulnerabilities", {})
        require(type(dependencies.get("total")) is int and dependencies["total"] > 0,
                "empty dependency inventory")
        require(all(type(counts.get(k)) is int and counts[k] >= 0
                    for k in ("info", "low", "moderate", "high", "critical", "total")),
                "invalid vulnerability counts")
        require(sum(counts[k] for k in ("info", "low", "moderate", "high", "critical"))
                == counts["total"], "vulnerability count mismatch")
        rows = data.get("vulnerabilities")
        require(isinstance(rows, dict) and len(rows) == counts["total"],
                "missing vulnerability records")
        count = counts["total"]
    else:
        database, lock = data.get("database", {}), data.get("lockfile", {})
        require(type(database.get("advisory-count")) is int
                and database["advisory-count"] > 0, "empty advisory database")
        require(type(lock.get("dependency-count")) is int
                and lock["dependency-count"] > 0, "empty lock inventory")
        vulns = data.get("vulnerabilities", {})
        rows, count = vulns.get("list"), vulns.get("count")
        require(isinstance(rows, list) and type(count) is int and count == len(rows)
                and vulns.get("found") is bool(count), "invalid Rust advisory inventory")
        warnings = data.get("warnings")
        require(isinstance(warnings, dict), "missing Rust warning inventory")
        require(all(isinstance(v, list) for v in warnings.values()), "invalid Rust warnings")
        count += sum(len(v) for v in warnings.values())
    require(status == 0 or count > 0, "nonzero exit without findings")
    return {"schema": 1, "tool": kind, "report_valid": True,
            "scanner_exit": status, "finding_count": count}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kind", choices=("cargo", "npm"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--strict", action="store_true")
    args, command = parser.parse_known_args()
    if command and command[0] == "--":
        command = command[1:]
    try:
        require(bool(command), "missing audit command")
        args.output.mkdir(parents=True, exist_ok=True)
        command[0] = shutil.which(command[0]) or command[0]
        completed = subprocess.run(command, capture_output=True, timeout=600, check=False)
        (args.output / "audit.json").write_bytes(completed.stdout)
        (args.output / "audit.stderr.log").write_bytes(completed.stderr)
        (args.output / "audit.exit").write_text(str(completed.returncode), encoding="utf-8")
        data = json.loads(completed.stdout)
        report = classify(args.kind, data, completed.returncode)
        (args.output / "validated.json").write_text(json.dumps(report) + "\n", encoding="utf-8")
        if os.environ.get("GITHUB_STEP_SUMMARY"):
            with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as stream:
                stream.write("### " + args.kind + " audit\n\n")
                stream.write("Validated findings: **" + str(report["finding_count"])
                             + "**; raw scanner exit: " + str(completed.returncode)
                             + ". Full JSON is retained in the artifact.\n")
                stream.write("Findings are advisory; execution/parse/coverage/upload failures are not.\n")
        print(json.dumps(report))
        return completed.returncode if args.strict else 0
    except (OSError, ValueError, TypeError, KeyError, subprocess.TimeoutExpired):
        print("::error::Dependency audit execution or report validation failed")
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
