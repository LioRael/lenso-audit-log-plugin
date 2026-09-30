#!/usr/bin/env python3
"""Verify retained normalized archives from the exact clean release source."""
import argparse
import hashlib
import json
import pathlib
import tarfile
import tomllib


def inspect(path, package, source_sha):
    name, version = package["package_name"], package["version"]
    prefix = f"{name}-{version}/"
    with tarfile.open(path, "r:gz") as archive:
        def read(relative):
            member = archive.extractfile(prefix + relative)
            if member is None:
                raise ValueError(f"missing {relative}")
            return member.read()
        manifest = tomllib.loads(read("Cargo.toml").decode())
        if manifest["package"]["name"] != name or manifest["package"]["version"] != version:
            raise ValueError("archive package identity differs from approved set")
        vcs = json.loads(read(".cargo_vcs_info.json"))
        if vcs["git"]["sha1"] != source_sha or vcs["git"].get("dirty", False):
            raise ValueError("archive was not produced from the exact clean source")
        return {**package, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                "source_sha": source_sha, "path_in_vcs": vcs["path_in_vcs"],
                "normalized_manifest_sha256": hashlib.sha256(read("Cargo.toml")).hexdigest(),
                "original_manifest_sha256": hashlib.sha256(read("Cargo.toml.orig")).hexdigest()}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--release-set", required=True)
    parser.add_argument("--directory", required=True, type=pathlib.Path)
    parser.add_argument("--output", required=True, type=pathlib.Path)
    args = parser.parse_args()
    packages = json.loads(args.release_set)
    records = [inspect(args.directory / f"{p['package_name']}-{p['version']}.crate", p, args.source_sha) for p in packages]
    args.output.write_text(json.dumps({"source_sha": args.source_sha, "archives": records}, indent=2) + "\n")


if __name__ == "__main__":
    main()
