"""python -m screencast [--port 7111] [--reprobe] [--no-browser]   control UI at http://127.0.0.1:PORT/
python -m screencast caps [--reprobe]                            print what this PC and the LAN's receivers can do
"""
import argparse
import asyncio
import json
import sys
import webbrowser
from dataclasses import asdict

from aiohttp import web


def main(argv=None):
    ap = argparse.ArgumentParser(prog="python -m screencast", description="Cast a Windows display to an Apple TV as live HLS.")
    ap.add_argument("command", nargs="?", choices=["serve", "caps"], default="serve")
    ap.add_argument("--port", type=int, default=7111)
    ap.add_argument("--reprobe", action="store_true", help="re-run the encoder probe instead of using the cache")
    ap.add_argument("--no-browser", action="store_true")
    a = ap.parse_args(argv)

    from . import app, receiver

    if a.command == "caps":
        m = app.load_machine(a.reprobe)
        rxs = asyncio.run(receiver.scan())
        json.dump({"displays": [asdict(d) for d in m.displays], "encoders": [asdict(e) for e in m.encoders],
                   "audio": [x.name for x in m.audio_sources], "notes": list(m.notes),
                   "receivers": [{"name": r["name"], "address": r["address"], "probed": r["probed"],
                                  **receiver.caps_json(r["caps"])} for r in rxs]}, sys.stdout, indent=2)
        print()
        return

    url = f"http://127.0.0.1:{a.port}/"
    print(f"open-airplay screencast: detecting this PC and scanning for receivers, then serving {url}", flush=True)
    application = app.App(a.port, a.reprobe).build()
    if not a.no_browser:
        async def open_browser(_):
            asyncio.get_running_loop().call_later(0.5, webbrowser.open, url)

        application.on_startup.append(open_browser)
    web.run_app(application, host="0.0.0.0", port=a.port, print=lambda *_: print(f"ready: {url}", flush=True))


if __name__ == "__main__":
    main()
