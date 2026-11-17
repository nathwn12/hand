# Third-party notices

hand incorporates source from two MIT-licensed projects, both owned by the
same copyright holder. They are compiled into one executable; no runtime
dependency on either installed product is required.

## Gladius

- Version: 0.8.0
- Pinned commit: `59c6d77`
- Repository: https://github.com/nathwn12/gladius
- License: MIT
- Vendored into: `crates/browser-engine/src`, plus the pin manifest and
  provisioning script under `crates/browser-engine/scripts`
- Copyright: Copyright (c) 2026 nathan
  <137482711+nathwn12@users.noreply.github.com>

The browser engine's MIT license text is reproduced in this repository's
`LICENSE` (same holder and terms). Vendored files keep their original contents;
the only changes are the public library surface and integration seams described
in `docs/COMPATIBILITY.md`.

## Puppet

- Version: 8.0.0
- Pinned commit: `0f790be`
- Repository: https://github.com/nathwn12/puppet
- License: MIT
- Vendored into: `crates/desktop-engine/src`
- Copyright: Copyright (c) 2026 nathan
  <137482711+nathwn12@users.noreply.github.com>

Same license text and same integration-seam note as above.

## Chromium

The owned browser artifact is `chrome-headless-shell`, provisioned separately
and not redistributed in this repository. Its version and zip digest are pinned
by `crates/browser-engine/scripts/chromium-manifest.json`; the local artifact
is verified against that pin before a live browser claim. Chromium is licensed
under its own terms (BSD-style and additional notices), not MIT.
