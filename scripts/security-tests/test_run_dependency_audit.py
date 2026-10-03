import copy
import sys
import unittest
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from run_dependency_audit import classify

class AuditContract(unittest.TestCase):
    def test_npm_clean_and_findings(self):
        data = {"auditReportVersion": 2, "metadata": {"dependencies": {"total": 2},
            "vulnerabilities": dict(info=0, low=0, moderate=0, high=0, critical=0, total=0)},
            "vulnerabilities": {}}
        self.assertEqual(classify("npm", data, 0)["finding_count"], 0)
        with self.assertRaises(ValueError):
            classify("npm", data, 1)
        data["metadata"]["vulnerabilities"].update(high=1, total=1)
        data["vulnerabilities"]["example"] = {"severity": "high"}
        self.assertEqual(classify("npm", data, 1)["finding_count"], 1)
        for bad in ({}, {"error": "offline"}, []):
            with self.assertRaises(ValueError):
                classify("npm", bad, 1)
        with self.assertRaises(ValueError):
            classify("npm", data, 2)

    def test_cargo_warnings_and_failures(self):
        data = {"database": {"advisory-count": 1}, "lockfile": {"dependency-count": 2},
                "vulnerabilities": {"found": False, "count": 0, "list": []}, "warnings": {}}
        self.assertEqual(classify("cargo", data, 0)["finding_count"], 0)
        data["warnings"] = {"unsound": [{"advisory": {"id": "RUSTSEC-example"}}]}
        self.assertEqual(classify("cargo", data, 1)["finding_count"], 1)
        for code in (2, 101, 124):
            with self.assertRaises(ValueError):
                classify("cargo", data, code)
        bad = copy.deepcopy(data)
        bad["lockfile"]["dependency-count"] = 0
        with self.assertRaises(ValueError):
            classify("cargo", bad, 0)

if __name__ == "__main__":
    unittest.main()
