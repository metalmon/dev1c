# Касперский и транспорт iroh: что ломается, что лечится без безопасника

Дата: 2026-11-01. Контекст: транспорт `1c-connect` — iroh, прямой путь по QUIC/UDP, запасной путь — собственный релей по HTTPS/WebSocket на 443. SSH из продукта убран, других путей нет. Вопрос владельца: можно ли работать за Касперским без обращения к безопасникам, и если да — чем это оплачено. В репозитории `1c-connect` зависимости на iroh в `Cargo.toml` пока нет (разведка перед решением), так что ниже — про штатные возможности крейтов, а не про то, что уже включено у нас.

## 1. Чем iroh проверяет сертификат релея

Факт (исходник `iroh-relay`, `src/tls.rs`, версия 1.0.0 на docs.rs, тот же код в `iroh`): модуль называется `CaTlsConfig` и имеет явно перечисляемые режимы:

```rust
impl CaTlsConfig {
    #[cfg(feature = "platform-verifier")]
    pub fn system() -> Self { ... }     // rustls_platform_verifier — хранилище ОС

    pub fn embedded() -> Self { ... }   // webpki_roots::TLS_SERVER_ROOTS — вшитые корни Mozilla
    pub fn custom_roots(roots: ...) -> Self { ... } // только явный список
}
```
— [iroh-relay 1.0.0 / src/tls.rs](https://docs.rs/crate/iroh-relay/1.0.0/source/src/tls.rs)

`CaTlsConfig::embedded()` — это умолчание. Прямое подтверждение из багрепорта самого проекта: «Dropping back to the default `CaTlsConfig::embedded()` makes the same relays connect immediately» — [iroh issue #4448](https://github.com/n0-computer/iroh/issues/4448). Значит, без вмешательства в код наш клиент будет сверяться со вшитым списком Mozilla, не с хранилищем Windows.

`CaTlsConfig::system()` существует, но закрыт Cargo-фичей `platform-verifier`, которая тянет опциональную зависимость:
```
rustls-platform-verifier = { version = "0.7.0", optional = true }
```
— [iroh-relay/Cargo.toml, main](https://github.com/n0-computer/iroh/blob/main/iroh-relay/Cargo.toml)

Это значит: переключение — не форк, а (а) включить фичу `platform-verifier` у зависимости `iroh`/`iroh-relay` и (б) один вызов в коде:
```rust
use iroh::tls::CaTlsConfig;
let endpoint = Endpoint::builder(presets::N0)
    .ca_tls_config(CaTlsConfig::system())
    .bind()
    .await?;
```
Это буквально рекомендация самого проекта для нашего сценария — цитата из официальной документации iroh (раздел про развёртывание в корпоративных сетях): «Use the operating system certificate store if you inspect TLS traffic» — и там же: «This affects the relay connection only. Direct connections between two devices identify each other by their own cryptographic keys rather than by website certificates, so an inspection appliance cannot sit in the middle of one. In practice it would never be in that path anyway.» — [docs.iroh.computer, llms-full.txt](https://docs.iroh.computer/llms-full.txt).

Следствие для нас: прямой путь (P2P QUIC) **не затрагивается** вопросом «вшитые корни или хранилище ОС» в принципе — там своя Ed25519-аутентификация по ключу узла, не цепочка CA. Вопрос верификации сертификата касается только релея (HTTPS/WebSocket), DNS-over-HTTPS и pkarr-публикации.

Это не гипотетический сценарий: issue [#2257](https://github.com/n0-computer/iroh/issues/2257) (2024, RHEL) — это ровно наш случай: `invalid peer certificate: UnknownIssuer`, потому что корпоративный корневой сертификат, которому доверяют macOS/curl/Safari, не входит во вшитый список Mozilla в rustls.

**Известный баг, который важен именно нам (Windows):** на Windows `rustls-platform-verifier` передавал CryptoAPI имя хоста с точкой на конце (у iroh релеи по умолчанию объявлены как `use1-1.relay.n0.iroh.link.` — абсолютные FQDN), и CryptoAPI сравнивал его буквально, получая `CERT_E_INVALID_NAME`/`NotValidForName` — тогда как webpki эту точку отбрасывает и не видит проблемы. Баг зафиксирован как [rustls/rustls-platform-verifier#240](https://github.com/n0-computer/iroh/issues/4448), исправлен в [PR #241](https://github.com/rustls/rustls-platform-verifier/pull/241), выпущен в версии 0.7.1 — issue закрыт 28 сентября 2026 с комментарием «There's now been a 0.7.1 release of rustls-platform-verifier. I think that closes this issue.» ([#4448](https://github.com/n0-computer/iroh/issues/4448)).

Следствие для нас: если включаем `platform-verifier`, нужно явно зафиксировать `rustls-platform-verifier >= 0.7.1` (semver `0.7.0` это разрешает, но лучше закрепить в `Cargo.lock` и перепроверить при обновлении) — иначе на Windows с хостнеймами с точкой на конце словим именно этот баг.

**Что не проверено:** не проверял, как называются релеи именно в нашей инсталляции (у нас может не быть «точки на конце» вообще, если мы не используем `presets::N0`/хосты n0 — тогда баг вовсе не актуален). Не нашёл в коде iroh способа переключать `CaTlsConfig` во время выполнения (runtime-настройкой/переменной окружения) — это компилируемый выбор, фиксируется при сборке эндпоинта.

## 2. Что Касперский делает с QUIC

Факт, повторяющийся в KB-статьях нескольких продуктов линейки (Kaspersky Internet Security, аналогично для Kaspersky Total Security/Anti-Virus): «Kaspersky Internet Security prevents data exchange over the QUIC protocol. Browsers use a standard transport protocol (TLS or SSL) regardless of whether or not support for the QUIC protocol is enabled in the browser.» — [Limitations and warnings, KIS](https://support.kaspersky.com/KIS/2018/en-US/85549.htm).

Это официальная документация, но она про потребительскую линейку (KIS/KTS), не про корпоративный Kaspersky Endpoint Security for Windows или шлюзовой Kaspersky Web Traffic Security — для них отдельного официального заявления про блокировку QUIC **не нашёл** (искал по support.kaspersky.com, попадаются только статьи про «Encrypted connections scan», которые перечисляют поддерживаемые протоколы как «SSL 3.0, TLS 1.0–1.3» — [Network settings, KESWin 12.0](https://support.kaspersky.com/KESWin/12.0/en-US/178483.htm) — QUIC в списке не упомянут вовсе, ни как сканируемый, ни как блокируемый явно).

Форумные (неавторитетные) подтверждения того, что это касается и актуальных версий: пользовательские темы на forum.kaspersky.com 2026 года спрашивают именно «когда будет поддержка HTTP/3 (QUIC)» для Kaspersky Basic/Standard/Plus/Premium — то есть по состоянию на 2026 год поддержки по-прежнему нет, но это форум, не документация.

Следствие для нас: official-документация подтверждает факт для потребительской линейки, и даёт основания полагать то же для корпоративной (архитектурно это тот же движок сканирования зашифрованных соединений, который и ограничен тем же списком протоколов TLS/SSL, без QUIC). Прямого официального подтверждения для KES for Windows / KWTS — нет, это пробел.

## 3. Что Касперский делает с WebSocket поверх HTTPS

Официально подтверждена механика MITM: «Kaspersky Endpoint Security adds a Kaspersky certificate to the system storage for trusted certificates and uses this certificate to scan encrypted connections» — [Scanning encrypted connections with an untrusted certificate](https://support.kaspersky.com/kes-for-windows/12.5/229683). Это прямое подтверждение посылки задачи: Касперский действительно встаёт посередине и действительно кладёт свой корень в системное хранилище Windows.

Там же — поведение при недоверенном сертификате у реального адресата: если сторонний процесс обращается к домену с недоверенным сертификатом, Касперский создаёт собственный сертификат со статусом «Untrusted» и далее действует по настройке «Allow» или «Block» (режим задаётся в блоке «Encrypted connections scan» → «Advanced settings»).

Прямого официального заявления про обработку апгрейда до WebSocket **не нашёл**. Нашёл только форумную (неавторитетную) тему «KIS blocks idle connections to websites websocket tests fail» ([forum.kaspersky.com](https://forum.kaspersky.com/topic/kis-blocks-idle-connections-to-websites-websocket-tests-fail-19953/)) — но там речь о таймауте простаивающих соединений, а не о структурной блокировке самого апгрейда или бинарного трафика внутри WS.

Официально задокументированное **ограничение**, которое теоретически может задеть наш релей: «due to technical limitations... scanning of encrypted connections does not support certain extensions of the TLS 1.0 protocol and later versions (particularly NPN and ALPN)» и «Kaspersky Internet Security does not support processing of HTTPS/2 Proxy traffic» — [Limitations and warnings, KIS](https://support.kaspersky.com/KIS/2018/en-US/85549.htm). Это гипотеза, не факт для нашего случая: если relay-клиент iroh договаривается по ALPN (`http/1.1` перед апгрейдом до WS), а движок сканирования Касперского плохо работает с ALPN — соединение может ограничиваться. Прямого подтверждения, что это бьёт именно по WebSocket-апгрейду iroh, не нашёл.

**Что не проверено:** нет официального источника о реакции Касперского конкретно на `Upgrade: websocket` заголовок внутри уже расшифрованного HTTPS; нет данных о реакции на бинарный трафик внутри открытого WS-канала специфично у Касперского (это общий риск для любого продукта DPI, не подтверждённый именно для Касперского).

## 4. Как решили соседи

**Tailscale** — задача почти идентична (DERP-релеи на HTTPS/443). Официальная документация прямо описывает наш сценарий и даёт официальную рекомендацию, требующую участия админа: «Disable or reduce the scope of any security profiles that perform SSL inspection» — для Fortinet, где перехват TLS к control plane даёт ошибку `x509: "controlplane.tailscale.com" certificate is not trusted` — [Using Tailscale with your firewall](https://tailscale.com/kb/1181/firewalls). То есть даже у Tailscale (Go, `crypto/tls` + системный пул по умолчанию на Windows) есть кейсы, где доверие системному хранилищу само по себе не спасает — потому что в этом случае не просто подмена сертификата, а разрыв соединения по категоризации трафика firewall'ом (App Control), а не по цепочке CA. Для чистого MITM-сценария (TLS inspection проходит, просто подменяет сертификат) OS-хранилище закрывает вопрос; для сценария «правило блокирует неопознанный трафик» — нет, и тут нужен админ, причём это универсально для любого инструмента, не специфично для iroh.

Отдельный баг, не относящийся к блокировке: «HTTPS DERP latency checks don't work correctly with proxy that does ssl mitm» ([issue #4377](https://github.com/tailscale/tailscale/issues/4377)) — MITM-проксирование путает логику замера задержки по Host-заголовку, это дефект самого Tailscale, не защитная мера Касперского-аналогов.

**Syncthing** — прямое заявление автора (calmh) о том, почему MITM в принципе не проходит на основном (не релейном) соединении: «Syncthing device IDs are essentially certificate pinning. It won't work to have a MITM proxy (by design)» — [форум Syncthing](https://forum.syncthing.net/t/how-does-syncthing-handle-its-end-to-end-security-issues-with-mitm-forti-ssl-corporate-firewall/20480). Но для релейного пути это не проблема: «it doesn't matter if it's MITM:ed, because we negotiate another TLS channel inside which the proxy should hopefully leave alone» (тот же источник) — то есть у Syncthing релей переживает подмену сертификата ровно потому, что внутри прокинут ещё один TLS-туннель, который инспектирующей стороне ломать уже бессмысленно/невозможно без знания ключей сторон. Та же тема прямо советует держать релей-сервер на 443 «to maximize connection chances» — [strelaysrv man page](https://manpages.ubuntu.com/manpages/plucky/man1/strelaysrv.1.html).

**ZeroTier** — официальной документации про поведение конкретно под антивирусной/корпоративной TLS-инспекцией **не нашёл**. Нашёл только нерелевантные форумные упоминания конфликтов ZeroTier с локальным файрволом Kaspersky Small Office Security (это про компонент «Firewall», а не про «Encrypted connections scan») — [discuss.zerotier.com](https://discuss.zerotier.com/t/zerotier-en-windows-server-2012-r2-como-controlador-de-dominio-con-kaspersky-small-office-security-instalado/5755). Косвенно релевантно замечание из документации ZeroTier про инспекцию в принципе: «Traditional firewalls and endpoint security tools only see encrypted ZeroTier traffic — they can't inspect or control what happens inside your virtual network» — [Rules Engine](https://docs.zerotier.com/rules/). ZeroTier не строит свой транспорт на CA-проверяемом TLS, поэтому подмена сертификата как класс атаки к нему просто неприменима — но это снимает только проблему №1 (сертификаты), не проблему блокировки незнакомого UDP-протокола как такового.

Общий паттерн у всех трёх: прямое (не релейное) соединение с взаимной аутентификацией по закреплённому ключу/отпечатку **органически несовместимо** с CA-based MITM — он туда просто не встаёт, не из-за защитного кода, а потому что подставному сертификату/ключу нет чем подтвердиться. Проблема целиком переносится на HTTPS/WebSocket-релей — ровно то же устройство, что у нас.

## 5. Приёмы без участия безопасников — и их цена

| Приём | Что даёт | Цена для нас |
|---|---|---|
| `CaTlsConfig::system()` + фича `platform-verifier`, зависимость `rustls-platform-verifier >= 0.7.1` | Релей доверяет тому же корню, что и Windows (включая корень, который Касперский сам туда ставит) — соединение проходит прозрачно, если Касперский в режиме Allow для пересканированных доменов | Низкая: штатный, документированный, не форк; нужно зафиксировать версию зависимости и один вызов в коде эндпоинта; не трогает прямой QUIC-путь вообще (там другая модель доверия) |
| Ничего не делать (оставить `embedded()`) | Прямой путь работает как есть; релей ломается с `UnknownIssuer`, как в [issue #2257](https://github.com/n0-computer/iroh/issues/2257), если Касперский инспектирует — упадём на релей с таким же разрывом, деградация без диагностируемой причины | Высокая по факту отказа: релей — наш единственный запасной путь, терять его из-за цепочки CA нелогично, когда есть готовый переключатель |
| HTTP/2 POST-потоки вместо WebSocket-апгрейда (на случай, если именно апгрейд режут) | Снимает теоретический риск блокировки `Upgrade: websocket` при разрешённом HTTPS | Высокая: протокол релея у iroh жёстко завязан на `tokio-websockets` ([Cargo.toml](https://github.com/n0-computer/iroh/blob/main/iroh-relay/Cargo.toml)) — это не настройка, а переписывание протокола клиента и сервера релея, то есть форк с нагрузкой сопровождения. Применять только если реально наблюдаем блокировку апгрейда, а не по гипотезе |
| Рандомизация исходного порта (аналог `randomizeClientPort` у Tailscale для Fortinet) | Может помочь, если блокировка идёт по фингерпринту «статичный порт = туннель» | Низкая, но у нас релей сидит на фиксированном 443 по архитектуре («фиксированные порты вместо умной логики» — так уже решено для деплоя), и к вопросу подмены сертификата это не относится вообще |
| Просить безопасника внести хост/приложение в исключения | Закрывает любой случай, включая блокировку по категоризации трафика (как у Fortinet+Tailscale) | Это и есть то, что владелец хочет избежать; работает всегда, но не бесплатно политически |

## Прямой ответ владельцу

Да, частично и программно — без обращения к безопасникам — закрывается именно та часть проблемы, которую описывает премиса вопроса (Касперский подменяет сертификат своим корнем в хранилище Windows): переключить проверку сертификата релея с вшитых корней Mozilla на системное хранилище Windows — штатная, не форкнутая возможность `iroh`/`iroh-relay` (`CaTlsConfig::system()` за Cargo-фичей `platform-verifier`, с зависимостью `rustls-platform-verifier >= 0.7.1` из-за бага с точкой на конце хостнейма на Windows, исправленного в сентябре 2026). Это прямо рекомендует сам проект для сценария «кто-то инспектирует TLS» — цитата из их документации приведена выше. Прямой QUIC-путь эту проблему не имеет в принципе: его аутентификация построена на ключе узла, а не на цепочке CA, так что подменять там нечего — но зато QUIC/UDP можно просто заблокировать как протокол (это задокументированное поведение Касперского для потребительской линейки, для корпоративной не нашёл отдельного подтверждения), и тогда мы автоматически падаем на тот самый релей — без участия админа, это штатный fallback.

Цена: 1) доверие системному хранилищу — это доверие и любому другому корню, который туда положил локальный администратор или другой агент, не только Касперскому; это осознанное ослабление, а не бесплатная опция. 2) Это лечит только случай, когда Касперский **пропускает** инспектированный трафик (режим Allow для непросканированных/недоверенных адресов) — если корпоративная политика стоит на Block для неопознанных приложений или гипотетически режет сам апгрейд до WebSocket (такое документировано как общий риск DPI, но не подтверждено конкретно для Касперского), программного обхода без администратора не существует — и это универсально: даже у Tailscale с его официальной документацией единственный ответ на такой случай — попросить администратора отключить или сузить SSL-инспекцию.
