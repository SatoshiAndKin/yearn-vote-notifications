"""Verify matching and drifting Rust toolchain pins without installing Rust."""

import unittest

from check_toolchain import check

VERSION = "1.100.0"
TOOLCHAIN = f'[toolchain]\nchannel = "{VERSION}"\nprofile = "minimal"\n'
DIGEST = "a" * 64
IMAGE = f"FROM rust:{VERSION}-slim-bookworm@sha256:{DIGEST} AS build\n"
RUNTIME = f"FROM debian:bookworm-slim@sha256:{DIGEST}\n"


class ToolchainCheck(unittest.TestCase):
    def test_check_matches_builder_and_ignores_runtime(self):
        self.assertEqual(check(TOOLCHAIN, IMAGE + RUNTIME), VERSION)
        self.assertEqual(
            check(TOOLCHAIN, IMAGE + RUNTIME.replace("bookworm", "trixie")), VERSION
        )

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
