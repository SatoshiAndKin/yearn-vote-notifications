"""Check the immutable Docker Rust builder matches rust-toolchain.toml."""

import re
import sys
import tomllib

from assemble_dockerfile import BUILDER, ROOT

STABLE_VERSION = r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
BUILD_FROM = re.compile(
    r"^\s*FROM[ \t]+(?:--platform=\S+[ \t]+)?(\S+)[ \t]+AS[ \t]+build[ \t]*$",
    re.IGNORECASE | re.MULTILINE,
)
RUST_IMAGE = re.compile(
    rf"(?:docker\.io/library/)?rust:({STABLE_VERSION})"
    r"(?:-[A-Za-z0-9][A-Za-z0-9_.-]*)?@sha256:[0-9a-f]{64}"
)


def check(toolchain: str, dockerfile: str) -> str:
    config = tomllib.loads(toolchain).get("toolchain", {})
    version = config.get("channel") if isinstance(config, dict) else None
    if not isinstance(version, str) or not re.fullmatch(STABLE_VERSION, version):
        raise ValueError("rust-toolchain.toml must pin an exact stable Rust version")
    bases = BUILD_FROM.findall(dockerfile)
    image = RUST_IMAGE.fullmatch(bases[0]) if len(bases) == 1 else None
    if image is None:
        raise ValueError("Dockerfile must have one digest-pinned official Rust build stage")
    if image[1] != version:
        raise ValueError(
            f"Rust pins differ: rust-toolchain.toml is {version}, Docker is {image[1]}; "
            "update both in one pull request"
        )
    return version


def main() -> None:
    try:
        version = check(
            (ROOT / "rust-toolchain.toml").read_text(),
            (ROOT / BUILDER).read_text(),
        )
    except (OSError, ValueError) as error:
        sys.exit(f"Rust toolchain check: {error}")
    print(version)


if __name__ == "__main__":
    main()
