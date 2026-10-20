# vendor/httpd

Apache HTTP Server 2.4.66 (Win64, Apache Lounge build), trimmed to the ~15
files `1c-connect`'s local web server needs: `httpd.exe` and the DLLs it
actually loads (`libhttpd`, `libapr-1`, `libaprutil-1`, `libapriconv-1`,
`libexpat`, `pcre2-8` — checked with `objdump -p`, nothing beyond those plus
the Windows-provided CRT/system DLLs), the five modules `mod_authz_core`,
`mod_authz_host`, `mod_alias`, `mod_mime`, `mod_dir`, and `conf/mime.types`
(httpd refuses to start without it). No `ab`/`htpasswd`/`ApacheMonitor` and
no openssl/curl/lua — this server never does TLS, auth, or scripting.

Lives here, not fetched by `pack.sh`, for the same reason as
`setup/dev1c-setup/vendor/nssm.exe`: 1.4 MB is cheap to vendor, and an Apache
Lounge download URL does not stay valid for long. `pack.sh` just copies this
tree into the kit as `httpd/`.

`LICENSE` and `NOTICE` are the canonical texts from the Apache HTTP Server
distribution (`apache/httpd` on GitHub) — Apache License 2.0, must travel
with the binaries.

Checked live before committing: a minimal conf loading only these five
modules, `httpd.exe -t` (syntax OK), then `-k start` / curl / `-k stop` —
served a file and shut down cleanly with no DLL errors.
