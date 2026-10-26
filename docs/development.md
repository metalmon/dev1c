# Development

Building a release, and the gates a change passes before it lands.

## Building a release

CI builds the image and the installers and uploads `dev1c-kit` with both kits: `dev1c-kit/` (the installer for Windows and Linux, `dev1c.toml`, the image, checksums) and `1c-connect/` (the Windows `mcp-1c` and its `.cfe` for the machine with the platform). Locally, `pack.sh <image.tar.gz> <bins>` assembles the same from already built parts; nothing is compiled on the laptop.

The model key never reaches the image: it is passed as an environment variable at run time, so every build is clean by construction rather than "remember to build without the key".

Building needs about 4 GB of RAM and 10 GB of disk; the build context is 2.4 MB and everything else is downloaded. No GPU required — the CUDA runtime is only unpacked as files.

## Formatting and lints

Formatting and lints are gated, not advisory: before the tests CI runs `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings`, on Windows and on Linux both, so the `#[cfg(windows)]` and `#[cfg(unix)]` halves are each looked at. The formatting half also runs locally before every push, in a second, once the repository's own hook is enabled:

```sh
git config core.hooksPath .githooks
```
