import importlib.util
import io
import json
import pathlib
import sys
import tarfile
import tempfile
import unittest

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("archives", pathlib.Path(__file__).with_name("release-archives.py"))
archives = importlib.util.module_from_spec(spec)
spec.loader.exec_module(archives)


class ArchivesTest(unittest.TestCase):
    def test_exact_source_and_manifests(self):
        package = {"package_name": "fixture", "version": "0.1.0"}
        sha = "1" * 40
        with tempfile.TemporaryDirectory() as directory:
            archive = pathlib.Path(directory) / "fixture.crate"
            def write(vcs, manifest_version="0.1.0"):
                with tarfile.open(archive, "w:gz") as output:
                    files = {"Cargo.toml": f'[package]\nname="fixture"\nversion="{manifest_version}"\n', "Cargo.toml.orig": "original", ".cargo_vcs_info.json": json.dumps(vcs)}
                    for name, content in files.items():
                        content = content.encode()
                        member = tarfile.TarInfo("fixture-0.1.0/" + name)
                        member.size = len(content)
                        output.addfile(member, io.BytesIO(content))
            write({"git": {"sha1": sha}, "path_in_vcs": "crates/fixture"})
            record = archives.inspect(archive, package, sha)
            self.assertEqual(record["path_in_vcs"], "crates/fixture")
            self.assertEqual(len(record["original_manifest_sha256"]), 64)
            for git in ({"sha1": "2" * 40}, {"sha1": sha, "dirty": True}):
                write({"git": git, "path_in_vcs": "crates/fixture"})
                with self.assertRaisesRegex(ValueError, "exact clean source"):
                    archives.inspect(archive, package, sha)
            write({"git": {"sha1": sha}, "path_in_vcs": "crates/fixture"}, "0.1.1")
            with self.assertRaisesRegex(ValueError, "identity"):
                archives.inspect(archive, package, sha)


if __name__ == "__main__":
    unittest.main()
