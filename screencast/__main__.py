"""python -m screencast [--port 7111] [--reprobe] [--no-browser]   control UI at http://127.0.0.1:PORT/
python -m screencast caps [--reprobe]                            print what this PC and the LAN's receivers can do
python -m screencast cast --receiver NAME [--display N] [--seconds S]   cast without the UI (Ctrl+C stops)
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
    ap.add_argument("command", nargs="?", choices=["serve", "caps", "cast"], default="serve")
    ap.add_argument("--port", type=int, default=7111)
    ap.add_argument("--reprobe", action="store_true", help="re-run the encoder probe instead of using the cache")
    ap.add_argument("--no-browser", action="store_true")
    ap.add_argument("--receiver", help="cast: receiver name or identifier")
    ap.add_argument("--display", type=int, default=None, help="cast: display index (default: primary)")
    ap.add_argument("--seconds", type=float, default=0, help="cast: stop after this long (0 = until Ctrl+C)")
    ap.add_argument("--latency", choices=["low", "balanced", "quality"], help="cast: latency mode (default: balanced)")
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

    if a.command == "cast":
        asyncio.run(cast(a))
        return

    url = f"http://127.0.0.1:{a.port}/"
    print(f"open-airplay screencast: detecting this PC and scanning for receivers, then serving {url}", flush=True)
    application = app.App(a.port, a.reprobe).build()
    if not a.no_browser:
        async def open_browser(_):
            asyncio.get_running_loop().call_later(0.5, webbrowser.open, url)

        application.on_startup.append(open_browser)
    web.run_app(application, host="0.0.0.0", port=a.port, print=lambda *_: print(f"ready: {url}", flush=True))


async def cast(a):
    from . import app

    core = app.App(a.port, a.reprobe)
    runner = web.AppRunner(core.build())
    await runner.setup()  # runs startup: detects this PC, scans receivers
    await web.TCPSite(runner, "0.0.0.0", a.port).start()
    try:
        display = a.display
        if display is None:
            display = next(d.index for d in core.machine.displays if d.primary)
        code, status = await core.begin({"receiver": a.receiver, "display": display,
                                          "overrides": {"latency": a.latency}})
        if code == 409:
            print("This receiver needs one-time pairing: run `python -m screencast` and start a cast from the UI.")
            return
        if code != 200:
            print("could not start:", json.dumps(status, indent=1))
            return
        s = status["settings"]
        print(f"casting display {display} -> {status['receiver']}: {s['codec']} {s['width']}x{s['height']}@{s['fps']} "
              f"{s['bitrate_kbps'] // 1000} Mb/s, stream {status['url']}", flush=True)
        loop = asyncio.get_running_loop()
        end = loop.time() + a.seconds if a.seconds else None
        while end is None or loop.time() < end:
            await asyncio.sleep(2)
            st = core.session.status()
            print(f"  state={st['receiver_state']} segments_fetched={st['segments_fetched']} "
                  f"encoder={st['stats'].get('fps')}fps error={st['error']}", flush=True)
            if st["error"]:
                break
    except KeyboardInterrupt:
        pass
    finally:
        if core.session:
            await core.session.stop()
        await runner.cleanup()


if __name__ == "__main__":
    main()
