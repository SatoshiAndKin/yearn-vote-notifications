"""Verify matching and drifting Rust toolchain pins without installing Rust."""

import re
import unittest

import assemble_dockerfile as dockerfile
from check_toolchain import check

VERSION = "1.100.0"
TOOLCHAIN = f'[toolchain]\nchannel = "{VERSION}"\nprofile = "minimal"\n'
DIGEST = "a" * 64
IMAGE = f"FROM rust:{VERSION}-slim-bookworm@sha256:{DIGEST} AS build\n"
RUNTIME = f"FROM debian:bookworm-slim@sha256:{DIGEST}\n"


class ToolchainCheck(unittest.TestCase):
    def test_check_matches_builder(self):
        self.assertEqual(check(TOOLCHAIN, IMAGE), VERSION)

    def test_independent_debian_digest_update(self):
        runtime = (dockerfile.ROOT / dockerfile.RUNTIME).read_text()
        builder = (dockerfile.ROOT / dockerfile.BUILDER).read_text()
        pin = re.search(r"^FROM debian:[^@\s]+@sha256:([0-9a-f]{64})$", runtime, re.M)
        self.assertIsNotNone(pin)
        old_digest = pin[1]
        new_digest = ("b" if old_digest == "a" * 64 else "a") * 64
        updated = runtime.replace(old_digest, new_digest)
        self.assertEqual(
            dockerfile.assemble(builder, updated),
            dockerfile.assemble(builder, runtime).replace(old_digest, new_digest),
        )
        self.assertNotIn(old_digest, builder)

    def test_assembly_joins_real_builder_and_runtime(self):
        builder = (dockerfile.ROOT / dockerfile.BUILDER).read_text()
        runtime = (dockerfile.ROOT / dockerfile.RUNTIME).read_text()
        product = dockerfile.assemble(builder, runtime)
        self.assertEqual(
            product, builder + "\n" + runtime.removeprefix(dockerfile.RUNTIME_PLACEHOLDER)
        )
        self.assertNotIn(dockerfile.RUNTIME_PLACEHOLDER, product)
        self.assertEqual(product.count(" AS build\n"), 1)
        self.assertIn("COPY --from=build /app/target/release/yearn-vote-notifications", product)

    def test_assembly_rejects_missing_placeholder(self):
        with self.assertRaisesRegex(ValueError, "empty build stage"):
            dockerfile.assemble(IMAGE, RUNTIME)

    def test_each_independent_bump_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "Rust pins differ"):
            check(TOOLCHAIN.replace(VERSION, "1.101.0"), IMAGE)
        with self.assertRaisesRegex(ValueError, "Rust pins differ"):
            check(TOOLCHAIN, IMAGE.replace(VERSION, "1.101.0"))

    def test_coordinated_bump_is_accepted(self):
        self.assertEqual(
            check(
                TOOLCHAIN.replace(VERSION, "1.101.0"),
                IMAGE.replace(VERSION, "1.101.0"),
            ),
            "1.101.0",
        )

    def test_unpinned_or_ambiguous_builder_is_rejected(self):
        for dockerfile in (
            IMAGE.replace(f"@sha256:{DIGEST}", ""),
            IMAGE.replace("rust:", "untrusted/rust:"),
            IMAGE.replace("AS build", "AS other"),
            IMAGE + IMAGE,
        ):
            with self.subTest(dockerfile=dockerfile):
                with self.assertRaisesRegex(ValueError, "digest-pinned"):
                    check(TOOLCHAIN, dockerfile)

    def test_moving_or_invalid_toolchain_is_rejected(self):
        for config in ("", TOOLCHAIN.replace(VERSION, "stable"), "toolchain = 42"):
            with self.subTest(config=config):
                with self.assertRaisesRegex(ValueError, "pin an exact"):
                    check(config, IMAGE)


if __name__ == "__main__":
    unittest.main()
