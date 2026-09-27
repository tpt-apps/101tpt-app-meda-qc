# Gumroad launch checklist

Status: **not ready for a paid launch**. The desktop shell and an unsigned
release-artifact workflow are implemented, but signing, notarization,
dependency packaging, licensing decisions and private-beta validation remain
open. Revisit item-by-item before the first upload.

## Blocking

- [x] Tauri desktop app implemented (native Tauri 2 shell with dashboard,
      import, local queue, inspector, timeline/evidence and report export).
- [ ] Decide on licensing model before the store listing goes up. Current
      state: dual MIT/Apache-2.0 (fully permissive) on a public-looking
      GitHub org (`tpt-apps`), while `CONTRIBUTING.md` calls this the
      "commercial application tier." If the repo stays public under this
      license, anyone can legally build and redistribute it for free — decide
      whether that's acceptable (paying for convenience/support) or whether
      the repo needs to go private / license needs to change first. Deferred
      as of 2026-09-25; revisit before launch.
- [ ] `ffprobe` dependency: the app shells out to a system `ffprobe` binary
      (`crates/tpt-app-media-qc-cli/src/probe.rs`). Either bundle a static
      ffmpeg/ffprobe binary with the installer, or clearly document it as a
      required separate install — customers won't have it by default.

## Build & packaging

- [x] Cross-platform **unsigned** release builds: Windows `.msi`, macOS `.dmg`,
      and Linux `.deb`/`.AppImage` on native runners.
- [ ] Code signing / notarization for Windows (Authenticode) and macOS
      (Apple Developer ID) — unsigned installers trigger SmartScreen/Gatekeeper
      warnings that kill conversion on a paid product.
- [x] Versioned **unsigned** release artifacts wired to CI: pushing a `v*` tag
      matching the Tauri configuration version builds Windows/macOS/Linux bundles
      and attaches them to a draft GitHub Release (`.github/workflows/release.yml`).
      Signing, notarization and Gumroad upload remain separate blockers.
- [ ] Confirm the git dependencies on `tpt-solutions/tpt-kinetix` and
      `tpt-solutions/tpt-cadence` are reachable by the CI/release runner — if
      those repos are private, the runner needs a deploy key or PAT with
      access, or release builds will fail to fetch them.

## Gumroad listing content

- [ ] Product title, one-line pitch, and description (pull from `spec.txt` /
      README feature list — content-based fingerprinting, rule-based QC
      profiles, deterministic reports).
- [ ] Screenshots/demo video of the actual GUI (the Tauri shell now exists;
      capture and approve store-ready assets before launch).
- [ ] Pricing tier(s) — one-time license vs. subscription; single-seat vs.
      studio/team.
- [ ] License key / activation mechanism if you want to gate paid use (Gumroad
      can auto-generate license keys; decide whether the app should validate
      one, given the permissive OSS license above may make this moot).
- [ ] EULA / terms of sale, distinct from the MIT/Apache source license if
      keeping that license.
- [ ] Support channel (email, issue tracker) and refund policy statement.
- [ ] Changelog entry for the release version (`CHANGELOG.md` already exists
      — keep it current per release).

## Post-launch

- [ ] Update process: does the app check for updates, or is re-download from
      Gumroad the only path?
- [ ] Crash/error reporting opt-in, consistent with the "offline-first, no
      mandatory network access" positioning in the README.
