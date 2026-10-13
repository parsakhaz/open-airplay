"""Freezes the cast sidecar (oap_sender.py + pyatv) into sender/dist/oap-sender/ so the installed app
can cast on a PC without Python. The Windows bundle ships that folder as resources/sender/.

    sender\.venv\Scripts\python -m pip install -r sender\requirements-build.txt
    sender\.venv\Scripts\python sender\freeze.py

A folder build, not a one-file build: a one-file exe unpacks itself and runs Python as a second process,
which survives when the app stops the first one.
"""
from pathlib import Path

import PyInstaller.__main__

HERE = Path(__file__).resolve().parent

PyInstaller.__main__.run([
    str(HERE / "oap_sender.py"),
    "--name", "oap-sender",
    "--onedir",
    "--console",  # the sidecar speaks on stdin/stdout; the app starts it without a window
    "--noconfirm",
    "--clean",
    "--copy-metadata", "pyatv",  # the ready line reports pyatv's version
    "--collect-submodules", "pyatv",  # protocols are imported by name at runtime
    "--distpath", str(HERE / "dist"),
    "--workpath", str(HERE / "build"),
    "--specpath", str(HERE / "build"),
])
