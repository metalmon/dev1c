# Security

## Reporting

Report privately, not in a public issue: open the repository's **Security** tab and choose **Report a vulnerability**. That creates a private advisory visible only to the maintainers.

If private reporting is unavailable to you, open a public issue that says only that you have a security report and asks for a contact — no details — and we will come to you.

We will acknowledge a report and tell you what we think of it. We cannot promise a fix window: this is a pilot product with one maintaining team.

## What this software is, in security terms

Read this before deciding whether something is a vulnerability. Several properties that look like findings are known, deliberate, and written down in the code next to the decision.

**The pilot's credentials are weak on purpose.** The three workstation logins `dev1`, `dev2`, `dev3` each have a password equal to the login, and the tunnel account is `dev`/`dev`, one shared pair for everyone. The environment is meant to sit on a trusted network or a developer's own machine, reachable on four ports, and the pilot traded credential hygiene for getting three people working on the first day. Reporting "default passwords" tells us nothing we do not know; reporting a path from those credentials to something they were never meant to reach is valuable.

**The tunnel account is deliberately narrow.** It has no shell and no sudo: sshd grants it port forwarding onto the nine workstation ports and nothing else, and the host key is remembered in the kit on first connect and must match afterwards. A way out of that account — a shell, a file, a port outside the nine — is a real finding. The reasoning for not requiring keys in the pilot is in `config/sshd-setup.sh`.

**The licence check is not anti-tamper, and is not even compiled in by default.** `setup/1c-connect/src/licence.rs` is a stub with one hard-coded date, it says in its own header that it does not try to stop a determined person from changing the clock, and it only guards anything when built with `--features licence`. Bypassing it is not a vulnerability. If it ever becomes a real licence, this paragraph changes with it.

**Model keys never enter the image.** They are passed as environment variables at run time, so a published image cannot carry one. A key found baked into a built image, or written into a volume or a log, is a real finding.

**The 1C platform is outside this software.** On the tunnelled setup it lives on the developer's own machine and is reached through the tunnel; nothing here installs or patches it. Vulnerabilities in 1C:Enterprise itself belong to 1C, not here — though we do want to hear if this software exposes the platform in a way its own configuration would not.

## In scope

- Anything that lets one workstation reach another's home directory, keyring, bases or Gitea account.
- Anything that escapes the tunnel account's forwarding restriction.
- Anything in the installer or the connector that writes outside its own kit and volumes, or that runs with privileges it was not given.
- A secret — model key, password, host key, token — that ends up in an image layer, a published artifact, a volume or a log.
- A kit that passes `dev1c-setup verify` while its contents have been altered.

## Out of scope

- The pilot credentials described above, used as designed.
- Bypassing the licence stub.
- Denial of service by exhausting the container's memory ceiling: `DEV1C_MEMORY` is a documented limit, and on the limit the kernel kills processes inside. That is the design, not a flaw.
- Findings in third-party components that we only ship — report those upstream, and tell us so we can take the fixed version.
