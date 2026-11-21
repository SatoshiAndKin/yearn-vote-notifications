"""Combine the independently tracked Rust builder and Debian runtime."""

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BUILDER = Path("docker/build/Dockerfile")
RUNTIME = Path("Dockerfile")
RUNTIME_PLACEHOLDER = "FROM scratch AS build\n\n"


def assemble(builder: str, runtime: str) -> str:
    if not runtime.startswith(RUNTIME_PLACEHOLDER):
        raise ValueError("Runtime Dockerfile must begin with the empty build stage")
    return builder.rstrip("\n") + "\n\n" + runtime.removeprefix(RUNTIME_PLACEHOLDER)


def main() -> None:
    try:
        builder = (ROOT / BUILDER).read_text()
        runtime = (ROOT / RUNTIME).read_text()
        sys.stdout.write(assemble(builder, runtime))
    except (OSError, ValueError) as error:
        sys.exit(f"Dockerfile assembly: {error}")


if __name__ == "__main__":
    main()
