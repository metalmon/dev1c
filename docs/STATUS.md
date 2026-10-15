# Working state

A ledger for picking the work up mid-stream: what is done, what is verified by
running it rather than by reasoning, and what is next. Updated as things change;
the design rationale lives in `docs/superpowers/specs/`.

Last updated: 2026-10-15.

## Where it runs

| | |
|---|---|
| repository | https://github.com/metalmon/dev1c (public) |
| image | built in CI, artifact `dev1c-image` (~1328 MB) |
| llama.cpp base | `ghcr.io/metalmon/dev1c-llama:b11606`, built from `Dockerfile.llama` |
| local container | `dev1c`, ports 3390 RDP / 3391 Gitea / 3392 model / 3393 SSH |
| workstations | `dev1`, `dev2`, `dev3` — password equals login |
| tunnel account | `dev` / `dev`, port forwarding only, no shell |

Secrets live in `.env` (git-ignored). Currently pointed at a remote model, with
the local llama.cpp switched off — no GPU worth the name on this machine yet:

```
CUSTOM_API_URL, CUSTOM_API_KEY, CUSTOM_MODEL, LLAMA_ENABLED=0
```

## Verified by running it

- **Image builds in CI** and passes its own checks: services start, the model
  name reaches the editor settings for both providers, `oscript` runs Cyrillic
  code, skills are intact, no key in the image environment.
- **Local model on a GPU.** Qwen3.5-4B on a 3060 Ti: 85 tokens/s, answers
  correctly. The CUDA runtime had to be in `/opt/llama` itself — unpacked into a
  subdirectory it is invisible to `ld.so`, and llama.cpp silently falls back to
  the CPU at 4 tokens/s.
- **Remote model.** Provider `custom`, key and address from `.env`, substituted
  into the settings at start.
- **Reverse SSH tunnel.** The host forwards a port into the container; both root
  and `dev1` reach it. This is the scheme for talking to the 1C platform.
- **Skills:** 40, flat, valid frontmatter, `cf-init` produces valid XML with
  Cyrillic intact.

## Not verified

- **Payload over the tunnel.** The channel works; nothing useful runs on the
  other end yet. Next step: an agent on the host able to start the configurator
  and `vanessa-runner`.
- **ERP-scale configuration.** Debugging happens on the Бухгалтерия test base
  (`C:\Users\bucher\Documents\InfoBase`, platform 8.3.27.1786 at
  `C:\Program Files (x86)\1cv8`). Anything may be done to it.
- **The desktop itself** — theme, layout switching, `.os` highlighting, Thunar
  bookmarks. Needs eyes in an RDP session.
- **PRISM runs.** The benchmark is seeded into Gitea; nothing has been run yet.

## Known traps, each cost a build or an evening

- `/home` is a volume: it hides what the image put there. An account added in a
  later image has no home until the entrypoint creates it, and a root-owned home
  makes sshd refuse keys without saying why.
- Settings from a new image do not reach existing homes by themselves — the
  entrypoint refreshes them, tracked by a stamp of the shipped version.
- `/etc/skel` is never substituted into: that would burn the placeholders and
  make the environment impossible to repoint at another provider.
- `nvprune` only trims static archives. A finished `.so` cannot be cut, which is
  why rebuilding against fewer architectures saved almost nothing until cuBLAS
  was linked statically.
- `GGML_STATIC=ON` links everything statically and breaks the build outright;
  the one line in `ggml/src/ggml-cuda/CMakeLists.txt` is patched instead.
- Debian 13 rejects NVIDIA's SHA1-signed repository key, so the llama.cpp build
  stage runs on Debian 12.
- Libraries must not be picked by name: that left `libmtmd` behind and the
  server would not start. The build now runs `llama-server --version` to prove
  it works.
- `llama.cpp` divides `--ctx-size` between slots. With `--parallel 3` each
  workstation got a third, and this environment's prompt did not fit — the agent
  answered nothing, and only the server's own log said why.
- Windows `ssh.exe` refuses to sign with a key whose ACLs it dislikes
  (`we did not send a packet, disable method`). Not a problem for the pilot,
  where the tunnel uses a password.
- The launch script recreates the container from the image, so anything placed
  inside with `docker cp` is gone on the next start.

## Next

1. **Payload over the tunnel:** find or build what runs 1C on the host, and
   reach it from the agent. This is the last unproven part of the scheme.
2. **PRISM:** run what can be run, use it to debug the skills on real tasks.
3. **Skills:** descriptions are 17 KB in every request, more than half the
   system prompt. Shorten only after they are debugged — the description is the
   only thing the model picks a skill by.
4. **ERP:** after Бухгалтерия behaves.

## Commands

```sh
./dev1c.sh | .\dev1c.ps1          # start, stop, remove, status, logs
./pack.sh  | .\pack.ps1           # build a release into dist/

gh run list --limit 5             # CI
gh run download <id> -n dev1c-image -D <dir>
docker load -i <dir>/dev1c-image.tar.gz

# tunnel from the machine with the platform
ssh -p 3393 -N -R 7777:localhost:7777 dev@<host>
```
