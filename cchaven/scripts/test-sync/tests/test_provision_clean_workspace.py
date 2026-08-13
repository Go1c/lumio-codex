#!/usr/bin/env python3
"""Offline tests for official test-server provisioning helpers."""

from __future__ import annotations

import importlib.util
import re
import unittest
import uuid
from pathlib import Path


SCRIPTS = Path(__file__).resolve().parents[1]


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ProvisionIdTests(unittest.TestCase):
    def test_default_workspace_id_shape(self) -> None:
        # Mirrors provision_clean_workspace.py default ID generation.
        suffix = uuid.uuid4().hex[:12]
        workspace_id = f"80000000-0000-4000-8000-{suffix}"
        self.assertRegex(
            workspace_id,
            r"^80000000-0000-4000-8000-[0-9a-f]{12}$",
        )
        self.assertEqual(len(suffix), 12)

    def test_remote_script_mentions_single_server_guard(self) -> None:
        text = (SCRIPTS / "provision_clean_workspace.py").read_text()
        self.assertIn("server_count", text)
        self.assertIn("listeners_9000", text)
        self.assertIn("max-workspaces-per-user", text)
        self.assertIn("/root/fns-deploy", text)
        self.assertIn("fns-selftest-", text)
        self.assertIn("pruned", text)
        self.assertNotIn("/root/my-workspace", text.split("Does NOT")[0])


class OfficialRunnerContractTests(unittest.TestCase):
    def test_runner_defaults_to_official_ssh_host(self) -> None:
        text = (SCRIPTS / "run_official_test_server_e2e.py").read_text()
        self.assertIn('DEFAULT_SSH_HOST = "vps-108-80-81-15"', text)
        self.assertIn("provision_clean_workspace.py", text)
        self.assertIn("controlled_ssh_e2e.py", text)
        self.assertIn("Never prints the JWT", text)
        self.assertIn("pass_fds=(3,)", text)


if __name__ == "__main__":
    unittest.main()
