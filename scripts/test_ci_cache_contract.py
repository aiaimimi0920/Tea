"""Only the scanner installation can be skipped by the build cache."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = (ROOT / ".github/workflows/build-tea-release.yml").read_text(encoding="utf-8")


class CacheContractTests(unittest.TestCase):
    def test_pinned_tool_cache_does_not_cache_advisories(self):
        cache = WORKFLOW.split("- name: Cache cargo-audit", 1)[1].split(
            "- name: Install pinned cargo-audit", 1
        )[0]
        self.assertIn("path: ${{ runner.temp }}/cargo-audit-tool", cache)
        self.assertIn("${{ runner.os }}-${{ runner.arch }}-0.22.2-rust-1.95.0", cache)
        self.assertNotIn("~/.cargo", cache)
        install = WORKFLOW.split("- name: Install pinned cargo-audit", 1)[1].split(
            "- name: Audit Rust dependency locks", 1
        )[0]
        self.assertIn("cache-hit != 'true'", install)
        self.assertIn('--version 0.22.2 --root "$RUNNER_TEMP/cargo-audit-tool"', install)

    def test_audits_run_on_hits_and_still_gate_windows(self):
        audit = WORKFLOW.split("- name: Audit Rust dependency locks", 1)[1].split(
            "- name: Retain Rust audit reports", 1
        )[0]
        self.assertNotIn("if:", audit)
        self.assertIn('version=$("$RUNNER_TEMP/cargo-audit-tool/bin/cargo-audit" --version)', audit)
        self.assertIn('test "$version" = "cargo-audit 0.22.2"', audit)
        # The cargo subcommand reports "cargo-audit-audit"; query the binary itself.
        self.assertNotIn("cargo audit --version", audit)
        self.assertIn("--output .tmp/audit-root -- cargo audit --json", audit)
        self.assertIn("--file apps/desktop/src-tauri/Cargo.lock --json", audit)
        self.assertNotIn("--no-fetch", audit)
        self.assertIn("needs: test-linux-startup-lock", WORKFLOW)
        self.assertIn("verify-tea-release-package.ps1", WORKFLOW)
        self.assertNotIn("continue-on-error", WORKFLOW)


if __name__ == "__main__":
    unittest.main()
