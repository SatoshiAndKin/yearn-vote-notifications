"""Check that the official, immutable Docker builder matches native Rust."""

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VERSION = r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
FROM = re.compile(
    r"^[ \t]*FROM[ \t]+(?:--platform=\S+[ \t]+)?(\S+)(?:[ \t]+AS[ \t]+(\S+))?",
    re.IGNORECASE | re.MULTILINE,
)
RUST = r"(?:(?:docker\.io/)?library/)?rust"


def check(toolchain: str, dockerfile: str) -> str:
    config = tomllib.loads(toolchain).get("toolchain", {})
    version = config.get("channel") if isinstance(config, dict) else None
    if not isinstance(version, str) or not re.fullmatch(VERSION, version):
        raise ValueError("rust-toolchain.toml must pin an exact stable Rust version")
    refs = [(image, stage) for image, stage in FROM.findall(dockerfile)
            if stage.lower() == "build" or re.match(rf"^{RUST}(?:[:@]|$)", image)]
    if len(refs) != 1 or not re.fullmatch(
        rf"{RUST}:{re.escape(version)}-slim-bookworm@sha256:[a-f0-9]{{64}}", refs[0][0]
    ) or refs[0][1].lower() != "build":
        raise ValueError(f"Expected one digest-pinned official Rust build stage matching {version}")
    return version


if __name__ == "__main__":
    try:
        print(check((ROOT / "rust-toolchain.toml").read_text(), (ROOT / "Dockerfile").read_text()))
    except (OSError, ValueError) as error:
        sys.exit(f"Rust toolchain check: {error}")
