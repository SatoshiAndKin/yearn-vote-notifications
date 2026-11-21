"""Test exact-version parity with no network and no compiler install."""

import unittest

from check_toolchain import check

TOOLCHAIN = '[toolchain]\nchannel = "1.100.0"\nprofile = "minimal"\n'
BUILDER = f"FROM rust:1.100.0-slim-bookworm@sha256:{'a' * 64} AS build\n"
RUNTIME = f"FROM debian:bookworm-slim@sha256:{'b' * 64}\n"


class ToolchainCheck(unittest.TestCase):
    def test_coordinated_and_independent_debian_updates(self):
        self.assertEqual(check(TOOLCHAIN, BUILDER + RUNTIME), "1.100.0")
        self.assertEqual(check(TOOLCHAIN, BUILDER + RUNTIME.replace("b" * 64, "c" * 64)), "1.100.0")
        self.assertEqual(check(TOOLCHAIN.replace("1.100.0", "1.101.0"),
                               BUILDER.replace("1.100.0", "1.101.0") + RUNTIME), "1.101.0")

    def test_rejects_either_one_sided_update(self):
        for toolchain, builder in (
            (TOOLCHAIN.replace("1.100.0", "1.101.0"), BUILDER),
            (TOOLCHAIN, BUILDER.replace("1.100.0", "1.101.0")),
        ):
            with self.subTest(toolchain=toolchain, builder=builder), self.assertRaises(ValueError):
                check(toolchain, builder + RUNTIME)

    def test_rejects_floating_missing_foreign_or_extra_rust(self):
        for invalid in (
            BUILDER.replace(f"@sha256:{'a' * 64}", ""),
            BUILDER.replace("rust:", "untrusted/rust:"),
            BUILDER.replace(" AS build", " AS other"),
            BUILDER + BUILDER.replace(" AS build", " AS shadow"),
        ):
            with self.subTest(builder=invalid), self.assertRaises(ValueError):
                check(TOOLCHAIN, invalid + RUNTIME)
        with self.assertRaises(ValueError):
            check(TOOLCHAIN.replace("1.100.0", "stable"), BUILDER + RUNTIME)


if __name__ == "__main__":
    unittest.main()
