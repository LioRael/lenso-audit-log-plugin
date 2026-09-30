#!/usr/bin/env python3
"""Generate a release-plz config that processes only the approved stage."""
import argparse
import json
import pathlib
import tomllib


def generate(selected):
    scripts = pathlib.Path(__file__).parent
    policy = json.loads((scripts / "release-policy.json").read_text())
    allowed = {(p["package_name"], p["version"]) for p in policy["packages"]}
    identities = [(p["package_name"], p["version"]) for p in selected]
    if len(identities) != len(set(identities)) or not set(identities) <= allowed:
        raise ValueError("stage is outside the exact package/version allowlist")
    template = tomllib.loads((scripts.parent / "release-owner.toml").read_text())
    if template["workspace"]["release"] is not False:
        raise ValueError("unselected packages must be disabled")
    result = '[workspace]\nrelease = false\nrelease_always = true\nchangelog_update = false\ngit_tag_name = "{{ package }}@{{ version }}"\n'
    for name, _ in sorted(identities):
        result += f'\n[[package]]\nname = "{name}"\nrelease = true\n'
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--release-set", required=True)
    parser.add_argument("--output", required=True, type=pathlib.Path)
    args = parser.parse_args()
    args.output.write_text(generate(json.loads(args.release_set)))


if __name__ == "__main__":
    main()
