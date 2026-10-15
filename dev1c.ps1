# 1C development environment.
#
#   .\dev1c.ps1            start
#   .\dev1c.ps1 -Stop      stop
#   .\dev1c.ps1 -Remove    stop and drop data (asks for confirmation)
#   .\dev1c.ps1 -Status    what runs inside
#   .\dev1c.ps1 -Logs      container log
#
# One container for everyone: three workstations (dev1, dev2, dev3 — password
# equals login) plus shared services, a git server and a model server. The model
# occupies the whole GPU memory, so there is nowhere to keep a copy per person.
#
# Ports: four consecutive from 3390. Taken? Pick another start:
#   .\dev1c.ps1 -Port 4000

param(
    [switch]$Stop,
    [switch]$Remove,
    [switch]$Status,
    [switch]$Logs,
    [switch]$NoGpu,
    [switch]$DockerSocket,
    [int]$Port = 3390,
    [string]$Image = "",
    [string]$Memory = "24g"
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

$Container = if ($env:NAME) { $env:NAME } else { "dev1c" }
if (-not $Image) { $Image = if ($env:IMAGE) { $env:IMAGE } else { "oktaplus/dev1c:latest" } }

$Engine = $null
foreach ($candidate in @("docker", "podman")) {
    if (Get-Command $candidate -ErrorAction SilentlyContinue) { $Engine = $candidate; break }
}
if (-not $Engine) {
    Write-Host "Neither docker nor podman found." -ForegroundColor Red
    exit 1
}

if ($Status) {
    & $Engine ps --filter "name=$Container" --format "{{.Names}}  {{.Status}}"
    Write-Host ""
    # pgrep -x matches the process name exactly; -f would match the probe command
    # itself and every service would look alive.
    $probe = 'for s in gitea llama-server xrdp sshd; do if pgrep -x "$s" >/dev/null 2>&1; then printf "  %-14s running\n" "$s"; else printf "  %-14s stopped\n" "$s"; fi; done'
    $out = & $Engine exec $Container sh -c $probe 2>$null
    if ($out) { $out | ForEach-Object { Write-Host $_ } } else { Write-Host "  (container is not running)" }
    exit 0
}

if ($Logs) {
    & $Engine logs --tail 50 $Container
    exit 0
}

if ($Stop -or $Remove) {
    & $Engine rm -f $Container 2>$null | Out-Null
    if ($Remove) {
        # Volumes hold repositories, working files and history — dropped only on
        # explicit confirmation.
        Write-Host "Delete data (repositories, projects, settings)? [y/N]: " -NoNewline
        if ((Read-Host) -match '^[yY]') {
            foreach ($v in @("home", "gitea", "models")) {
                & $Engine volume rm "$Container-$v" 2>$null | Out-Null
            }
            Write-Host "Data deleted."
        } else { Write-Host "Data kept." }
    }
    Write-Host "Environment stopped."
    exit 0
}

# Without the image the engine would go to a registry, which in an air-gapped
# network looks like a network problem and misleads everyone.
if (-not (& $Engine image inspect $Image 2>$null)) {
    Write-Host "Image '$Image' not found. Load it from the release file:" -ForegroundColor Red
    Write-Host "  $Engine load -i dev1c-image.tar.gz"
    exit 1
}

& $Engine rm -f $Container 2>$null | Out-Null

# Key and address of the remote model are passed at run time instead of being
# baked into the image: every build stays clean, and the repository does not
# reveal what we run and where. See .env.example. In an air-gapped network .env
# is not needed at all — llama.cpp there wants neither key nor address.
$customVars = @{}
# LLAMA_ENABLED=0 turns off the local model server — useful while the machine
# has no GPU worth the name, or when a remote model is used instead.
foreach ($name in @("CUSTOM_API_URL", "CUSTOM_API_KEY", "CUSTOM_MODEL", "LLAMA_ENABLED",
                    "MCP_1C_BASE_URL", "MCP_1C_USER", "MCP_1C_PASSWORD")) {
    $value = [Environment]::GetEnvironmentVariable($name)
    if (-not $value -and (Test-Path "$PSScriptRoot\.env")) {
        $line = Get-Content "$PSScriptRoot\.env" |
                Where-Object { $_ -match "^$name=" } | Select-Object -First 1
        if ($line) { $value = ($line -split '=', 2)[1].Trim() }
    }
    if ($value) { $customVars[$name] = $value }
}

# Data lives in named volumes only. Bind-mounted host directories are unreliable
# on Windows: antivirus software keeps the files open and reads from inside the
# container fail mid-stream with I/O errors rather than a clear refusal. The
# model is uploaded into its volume once — see the hint printed at the end.
$runArgs = @(
    "run", "-d", "--name", $Container, "--restart", "unless-stopped",
    # A real init as PID 1. Without it PID 1 is xrdp, which neither reaps orphans
    # nor arranges sessions properly.
    "--init",
    "-p", "$($Port):3389",        # RDP, all three workstations on one port
    "-p", "$($Port + 1):3000",    # Gitea
    "-p", "$($Port + 2):8080",    # model server
    "-p", "$($Port + 3):2022",    # SSH for the reverse tunnel
    "--shm-size", "2g",
    "--memory", $Memory,
    # xrdp-sesman opens a PAM session: needs setsid and process management
    "--cap-add", "SYS_ADMIN",
    "--security-opt", "seccomp=unconfined",
    "--security-opt", "apparmor=unconfined",
    # Windows drive forwarding goes through FUSE; the device is not granted by default
    "--device", "/dev/fuse",
    "-v", "$($Container)-home:/home",
    "-v", "$($Container)-gitea:/var/lib/gitea",
    "-v", "$($Container)-models:/opt/models"
)

# The engine socket is not exposed inside: access to it equals root on the host,
# and nothing in this environment uses it. Enable explicitly if ever needed.
if ($DockerSocket) {
    $runArgs += @("-v", "/var/run/docker.sock:/var/run/docker.sock")
}

if (-not $NoGpu) {
    # GPU flags differ between engines
    if ($Engine -eq "podman") { $runArgs += @("--device", "nvidia.com/gpu=all") }
    else { $runArgs += @("--gpus", "all") }
}

# Passed through under the same names they have in .env — no renaming anywhere,
# so there is nothing to get wrong. The provider in the editor's settings is
# called "custom", and Zed derives the key variable from the provider name.
foreach ($pair in $customVars.GetEnumerator()) {
    $runArgs += @("-e", "$($pair.Key)=$($pair.Value)")
}

$runArgs += $Image

& $Engine @runArgs | Out-Null
if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "Start failed. Two usual reasons:" -ForegroundColor Yellow
    Write-Host "  ports $Port-$($Port + 3) are taken  ->  .\dev1c.ps1 -Port 4000"
    Write-Host "  no GPU or no driver                 ->  .\dev1c.ps1 -NoGpu"
    exit 1
}

Write-Host ""
Write-Host "1C development environment started ($Engine):" -ForegroundColor Green
Write-Host ""
Write-Host "  Workstations    mstsc /v:localhost:$Port   —  dev1 / dev1,  dev2 / dev2,  dev3 / dev3"
Write-Host "  Repositories    http://localhost:$($Port + 1)"
Write-Host "  Model server    http://localhost:$($Port + 2)"
Write-Host "  SSH for tunnel  port $($Port + 3)"
Write-Host ""

& $Engine exec $Container sh -c 'ls /opt/models/*.gguf' *> $null
if ($LASTEXITCODE -ne 0) {
    Write-Host "No model file — the agent will stay silent. Upload one once:" -ForegroundColor Yellow
    Write-Host "  $Engine cp model.gguf $($Container):/opt/models/"
    Write-Host "  $Engine restart $Container"
    Write-Host ""
}

# Explicit: otherwise the exit code is inherited from the model-file check and a
# fully started environment looks like a failure.
exit 0
