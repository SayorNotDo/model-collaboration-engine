import importlib.util
import json
import tempfile
import unittest
import subprocess
import sys
import contextlib
import io
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "scripts" / "scan_project.py"
SPEC = importlib.util.spec_from_file_location("scan_project", SCRIPT)
scan_project = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(scan_project)


class ScanProjectTests(unittest.TestCase):
    def test_detects_typescript_vue_and_package_scripts(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "src" / "App.vue").write_text("<template />", encoding="utf-8")
            (root / "tsconfig.json").write_text("{}", encoding="utf-8")
            (root / "vite.config.ts").write_text("export default {}", encoding="utf-8")
            (root / "package.json").write_text(json.dumps({
                "devDependencies": {"typescript": "*", "vue": "*", "vite": "*", "vitest": "*"},
                "scripts": {"lint": "eslint .", "typecheck": "vue-tsc --noEmit", "test": "vitest run"},
            }), encoding="utf-8")
            profile = scan_project.scan(root)
            self.assertEqual(profile["project"]["languages"], ["typescript"])
            self.assertEqual(profile["project"]["frameworks"], ["vite", "vue"])
            self.assertEqual(profile["quality"]["test_runner"][0]["command"], "npm run test")
            self.assertEqual(profile["quality"]["test_runner"][0]["name"], "vitest")

    def test_detects_python_tools(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "pyproject.toml").write_text(
                "[tool.ruff]\nline-length = 100\n[tool.pytest.ini_options]\ntestpaths = ['tests']\n[tool.mypy]\nstrict = true\n",
                encoding="utf-8",
            )
            profile = scan_project.scan(root)
            self.assertEqual(profile["project"]["languages"], ["python"])
            self.assertEqual(profile["quality"]["linter"][0]["name"], "ruff")
            self.assertEqual(profile["quality"]["typecheck"][0]["name"], "mypy")
            self.assertEqual(profile["quality"]["test_runner"][0]["name"], "pytest")

    def test_detects_rust_defaults(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text("[package]\nname='demo'\nversion='0.1.0'\n", encoding="utf-8")
            profile = scan_project.scan(root)
            self.assertEqual(profile["project"]["languages"], ["rust"])
            self.assertEqual(profile["quality"]["formatter"][0]["name"], "rustfmt")
            self.assertEqual(profile["quality"]["test_runner"][0]["command"], "cargo test")

    def test_existing_output_is_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / ".engineering" / "project-profile.yaml"
            output.parent.mkdir()
            output.write_text("keep: true\n", encoding="utf-8")
            result = scan_project.main([str(root), "--output", str(output)])
            self.assertEqual(result, 2)
            self.assertEqual(output.read_text(encoding="utf-8"), "keep: true\n")

    def scan_files(self, files):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, content in files.items():
                (root / name).write_text(content, encoding="utf-8")
            return scan_project.scan(root)

    def test_js_dependencies_without_scripts_are_detected_but_unresolved(self):
        profile = self.scan_files({"package.json": json.dumps({"devDependencies": {
            "typescript": "*", "eslint": "*", "prettier": "*", "vitest": "*"}})})
        for category, name in (("formatter", "prettier"), ("linter", "eslint"),
                               ("typecheck", "tsc"), ("test_runner", "vitest")):
            tool = profile["quality"][category][0]
            self.assertEqual(tool["name"], name)
            self.assertIsNone(tool["command"])
            self.assertEqual(tool["verification"], "inspect")

    def test_config_files_detect_tools_without_evaluating_code(self):
        profile = self.scan_files({"eslint.config.js": "throw new Error('do not execute')",
                                   "vitest.config.ts": "invalid javascript",
                                   ".prettierrc": "{}", "tsconfig.app.json": "{}"})
        self.assertEqual(profile["quality"]["linter"][0]["name"], "eslint")
        self.assertEqual(profile["quality"]["test_runner"][0]["name"], "vitest")
        self.assertEqual(profile["project"]["languages"], ["typescript"])

    def test_black_formatter_is_not_replaced_by_ruff_linter(self):
        profile = self.scan_files({"pyproject.toml": "[tool.ruff]\n[tool.black]\n"})
        self.assertEqual([t["name"] for t in profile["quality"]["formatter"]], ["black"])
        self.assertEqual([t["name"] for t in profile["quality"]["linter"]], ["ruff"])

    def test_ruff_lint_configuration_does_not_imply_formatter(self):
        profile = self.scan_files({"pyproject.toml": "[tool.ruff]\n"})
        self.assertEqual(profile["quality"]["formatter"], [])
        profile = self.scan_files({"pyproject.toml": "[tool.ruff.format]\n"})
        self.assertEqual(profile["quality"]["formatter"][0]["name"], "ruff")

    def test_python_dependency_groups(self):
        profile = self.scan_files({"pyproject.toml": '[dependency-groups]\ndev=["pytest>=8", "black", "mypy"]\n'})
        self.assertEqual(profile["quality"]["test_runner"][0]["name"], "pytest")
        self.assertEqual(profile["quality"]["formatter"][0]["name"], "black")

    def test_mixed_project_retains_each_stack_test(self):
        profile = self.scan_files({"package.json": json.dumps({"scripts": {"test": "vitest run"}}),
            "Cargo.toml": '[package]\nname="demo"\nversion="0.1.0"\n',
            "pyproject.toml": "[tool.pytest.ini_options]\n"})
        self.assertEqual({t["name"] for t in profile["quality"]["test_runner"]},
                         {"vitest", "pytest", "cargo-test"})

    def test_mutating_scripts_are_never_certified_as_read_only(self):
        profile = self.scan_files({"package.json": json.dumps({"packageManager": "pnpm@9.0.0",
            "scripts": {"format": "prettier --write .", "lint": "eslint --fix ."}})})
        self.assertEqual(profile["quality"]["formatter"][0]["command"], "pnpm run format")
        self.assertEqual(profile["quality"]["formatter"][0]["verification"], "inspect")
        self.assertEqual(profile["quality"]["linter"][0]["verification"], "inspect")

    def test_invalid_manifests_and_shapes_fail(self):
        for name, content in (("package.json", "{invalid"), ("package.json", "[]"),
                              ("package.json", '{"scripts": []}'),
                              ("pyproject.toml", "[broken"), ("Cargo.toml", "[broken"),
                              ("pyproject.toml", '[project]\ndependencies="pytest"')):
            with self.subTest(name=name, content=content), self.assertRaises(scan_project.ScanError):
                self.scan_files({name: content})

    def test_cli_from_other_working_directory_has_no_project_writes(self):
        with tempfile.TemporaryDirectory(prefix="project with spaces ") as directory:
            result = subprocess.run([sys.executable, "-B", str(SCRIPT.resolve()), directory],
                                    cwd=directory, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('"0.2.0"', result.stdout)
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_cli_parse_failure_creates_no_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "package.json").write_text("{invalid", encoding="utf-8")
            output = root / ".engineering" / "project-profile.yaml"
            with contextlib.redirect_stderr(io.StringIO()) as errors:
                self.assertEqual(scan_project.main([directory, "--output", str(output)]), 2)
            self.assertIn("package.json", errors.getvalue())
            self.assertFalse(output.parent.exists())

    def test_explicit_output_and_force(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / ".engineering" / "project-profile.yaml"
            self.assertEqual(scan_project.main([directory, "--output", str(output)]), 0)
            output.write_text("old", encoding="utf-8")
            self.assertEqual(scan_project.main([directory, "--output", str(output), "--force"]), 0)
            self.assertIn('"0.2.0"', output.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
