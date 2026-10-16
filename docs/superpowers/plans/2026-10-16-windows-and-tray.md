# Окна обеих программ и трей у 1c-connect (этап 4)

Спек: `docs/superpowers/specs/2026-10-16-setup-binaries-design.md`, раздел
«Окна и трей (этап 4)». Выполняется в этой же сессии, по шагам, коммит на шаг.

1. `common`: `Check` сериализуется (окно получает список фактов как есть);
   `Kit::journal(&Event)` — общая запись `Command`/`Output` в `setup.log`
   для обоих CLI и обоих окон; `console::attach()` — возврат консоли
   оконной сборке на Windows (копия `setup_console` из volt-setup).
2. `1c-connect`: модуль `session` (`State` с тестом переходов, `start`,
   `Session::stop`/`wait`, `serve` — мост из `main.rs`); `connect` через него.
   Проверка: `cargo test -p onec-connect --no-default-features`.
3. `1c-connect`: Tauri — `Cargo.toml` (`gui` по умолчанию, `tray-icon`,
   `image-png`), `build.rs`, `tauri.conf.json`, `capabilities/default.json`,
   `icons/`, `ui/`, `src/gui.rs` (команды, трей, значок со статусом — тест
   на угловой пиксель), `main.rs` без аргументов → окно.
4. `dev1c-setup`: то же без трея; `ui/` своя страница, `src/gui.rs`.
5. CI: Linux — `--no-default-features` для теста и сборок; Windows — как
   есть (окно). `.gitignore`: `setup/*/gen/`.
6. Прогон CI, запуск `1c-connect.exe` руками на этой машине (окно, трей,
   подключение к среде в CI недоступно — проверяется `check` и форма
   параметров), `docs/STATUS.md`.
