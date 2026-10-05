# Changelog

All notable changes to `tpt-app-media-qc-store` are documented here, per
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). For the full
product history see the workspace [`CHANGELOG.md`](../../CHANGELOG.md).

## [Unreleased]

### Added

- Initial changelog: SQLite store with versioned migrations; projects,
  assets, jobs, profiles, results, findings, report metadata and preferences;
  per-rule analysis cache keyed on fingerprint + app/ruleset versions +
  profile and rule hashes.
