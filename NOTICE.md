# Third-party notices and provenance

## doubletake sender core

The initial AirPlay sender protocol, capture abstraction, daemon, command-line
tools, and associated tests in this repository were imported from:

- Project: `omarroth/doubletake`
- Upstream: <https://github.com/omarroth/doubletake>
- Revision: `5e2f1cbe3e1070bcefec8e549c7baf884727bc00`
- Revision date: 2026-10-04
- License: GNU Lesser General Public License v3.0 or later

The LGPL text is included as `LICENSE`; the incorporated GNU GPLv3 text is
included as `COPYING.GPL`. Local changes, including the Windows capture
backend, display selection, naming, documentation, and tests, are tracked in
this repository's Git history.

The upstream project credits prior AirPlay reverse-engineering work including
UxPlay and related receiver projects. Its FairPlay SAP implementation contains
reverse-engineered white-box cryptographic material. FairPlay SAP authenticates
an AirPlay sender; it is not FairPlay Streaming DRM and does not decrypt
protected commercial media.

## Runtime dependencies

Go dependency versions and checksums are recorded in `go.mod` and `go.sum`.
The Windows capture backend invokes an installed FFmpeg build. FFmpeg and its
linked codecs are separately licensed by their respective authors and are not
vendored in this repository.
