# The demo stand: the platform inside the container

A second image for a customer who will install nothing. Everything below is the host side of building and moving it.

For a customer who will install nothing on their own machines there is a second image, `oktaplus/dev1c-1c`, built by `Dockerfile.1c` on top of `oktaplus/dev1c`: the **training** 1C:Enterprise 8.5 (file bases only, one Enterprise session per base, no server), a demo base per workstation, `mcp-1c` and `vanessa-runner` inside. The agent sees the same ports as with the tunnel — `777N` (mcp-1c), `778N` (vrunner-mcp) — served locally by `config/1c/stand.sh`, so Zed, the skills and the rules do not change; the Configurator is in the application menu, and `1cv8` is on the `PATH`. The base of workstation `devN` is `~/base`, a copy of the template base made on first use; the project directory for vrunner is `~/project`.

The platform and the template base are **not in the image**. They come in two named volumes, `dev1c-platform` (`/opt/1cv8t`) and `dev1c-demo` (`/opt/1c-demo`), filled once from a payload tar. They follow 1C's releases, not ours: baked in, every change to `stand.sh` meant moving 11 GB of build context, image and kit, and a customer who already had the platform downloaded it again. The image is about 4 GB, the payload about 4 GB more, and the payload survives image updates. Without the volumes the container still gives the desktop, Zed and Gitea, and `1c-stand start` says in one line what is missing.

The installer is 1.7 GB and comes from the 1C site through a browser form, so both are built where the file is, not in CI. `provision.sh` is the whole host side, with fixed names and no discovery:

```
mkdir platform                         # git- and docker-ignored
# put setup-training-8.5.1.1150-x86_64.run there (from training_8_5_1_1150_LinuxRun.tar.gz),
# unf-ready.dt and DoNotCopy.txt, the demo base of the stand (see below)
mkdir artifacts                        # 1c-connect (the Linux binary, CI artifact 1c-connect-linux)
                                       # missing -> the stand builds without the connector's window and says so
./provision.sh build                   # the stand image and the payload image
./provision.sh export                  # -> dist/dev1c-1c-payload.tar.gz, outside the build context
./provision.sh import                  # that tar -> the two volumes
./provision.sh run -e CUSTOM_API_URL=… -e CUSTOM_API_KEY=… -e CUSTOM_MODEL=…
./provision.sh status                  # what the volumes hold
docker exec dev1c test-stand           # the check of a running stand
```

Each target takes only the context it uses: the stand image reads `artifacts/` (68 MB), the payload reads `platform/` (3.8 GB). Mounting the big one in both cost eleven minutes per rebuild of the image that is meant to be quick to rebuild; now it is under a minute.

On another machine only the last three lines happen, carrying two files: the image (`docker save oktaplus/dev1c-1c:latest | gzip`) and `dev1c-1c-payload.tar.gz`. There is no packed stand kit yet — `pack.sh` builds the kit of the development environment, not of the stand. The payload is rebuilt only for a new platform or a new demo configuration; `payload.conf` inside it records which versions it was built from, and `test-stand` prints that line first.

`DEV1C_NAME` renames the container and all four volumes together and `DEV1C_PORT_BASE` moves the three ports, so a second stand can be raised beside a live one without touching it. `DEV1C_MEMORY` is the ceiling for the container as a whole, 8 GB by default: all three workstations sit in it together with Gitea, three copies of Zed, the analyzer and the 1C clients, and on the limit the kernel kills processes inside. The model server is a separate container and its memory is not counted here.

The demo base is the training УНФ 3.0.13.251 without data: the kit's demo base with data has users with passwords, and the training licence admits only empty ones. `unf-ready.dt` is that empty base with the configuration's welcome wizard already done. The wizard («Добро пожаловать!», three steps, a required name) cannot be skipped, and finishing it creates an infobase user — so if the customer meets it, the agent's connection breaks in the middle of the demo. Done once at build time instead, and the name of the user it makes (`DEMO_DB_USER`, «Администратор», empty password — the only kind the licence allows) is carried by the publication, by the base list of the starter and by vrunner. To make the file: build a base from `unf-empty.dt` (1cv8new.dt of the kit), open a copy of it, walk the three steps giving that name and no password, then `1cv8 DESIGNER /F <base> /NАдминистратор /DumpIB unf-ready.dt`. Building without a wizard-done template needs both arguments: `--build-arg DEMO_TEMPLATE=unf-empty.dt --build-arg DEMO_DB_USER=`.

`platform/DoNotCopy.txt` goes with that template. A BSP configuration keeps the identifier of its file base in that file, and a base restored from a `.dt` has no such file, so the first person in is asked whether the base was moved or is a copy, with external resources locked and `[КОПИЯ]` in the title until they answer. Make it once: restore `unf-ready.dt` into a base, open it, answer «Информационная база перемещена», and take the `DoNotCopy.txt` the configuration writes. The build drops it next to the template base and every workstation copy inherits it. Regenerate it with the template; a stale identifier only brings the question back.

The build itself checks the platform (ldd, a base created under Xvfb), the mcp-1c HTTP service on the demo base through a throwaway Apache, and vrunner-mcp's tool list. While a stand holds `777N`/`778N`, `1c-connect` from a real machine cannot bind them: a workstation is either demo or real.
