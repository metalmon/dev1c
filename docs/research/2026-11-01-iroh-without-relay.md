# iroh без релея: когда можно обойтись, когда нет

Дата: 2026-11-01. Контекст: `1c-connect` пока не зависит от iroh (в `Cargo.toml` проекта iroh не упоминается — это разведка перед решением). Источники — документация iroh (docs.iroh.computer), блог n0, issues/PR репозитория `n0-computer/iroh`. Везде, где факта не нашёл, написано прямо.

Важная терминологическая оговорка: в текущих (1.x) доках iroh переименовал `NodeAddr` → `EndpointAddr`, `NodeId` → `EndpointId`, модуль `discovery::local_swarm_discovery` → `discovery::mdns`. Старые примеры (iroh-net 0.28 и более старые блоги) используют старые имена — это один и тот же механизм, не два разных.

## 1. Знакомство (discovery)

| Способ | Крейт / механизм | Нужна ли инфраструктура n0 | По умолчанию |
|---|---|---|---|
| Явный адрес (ticket / `EndpointAddr` с прямыми адресами) | нет крейта, просто структура данных | **нет** | нет (адрес передаёт приложение) |
| mDNS в локальной сети | `iroh-mdns-address-lookup` (на крейте `swarm-discovery`, частота анонсов зависит от трафика) или `iroh-mdns-peer-lookup` (стандартный mDNS/DNS-SD) | **нет** | **нет**, в iroh 1.x оба крейта опциональны, их нужно добавлять явно |
| DNS/Pkarr глобальный дискавери | `dns.iroh.link`, сервер держит n0; пакет подписывается ключом узла (Pkarr) | да (или свой DNS/Pkarr-сервер) | да — включён в `presets::N0` |
| BitTorrent Mainline DHT | peer-to-peer lookup, крейт в составе iroh | нет (децентрализовано, но нужен выход в DHT-сеть) | **нет**, выключено по умолчанию (в т.ч. чтобы мобильные приложения не выглядели как BitTorrent-клиенты) |

Факт: «Local discovery is optional in iroh 1.x. Neither of these crates is enabled by default» — [mDNS](https://docs.iroh.computer/connecting/local-address-lookup).
Факт: DNS-дискавери по умолчанию указывает на `dns.iroh.link`, сервер n0 — [Dial by NodeID, no address required!](https://www.iroh.computer/blog/iroh-dns).
Факт: явный адрес работает без всякого discovery-сервиса: «Connections by explicit address still work» — [Deploying to enterprise networks](https://docs.iroh.computer/configuring-networks).

Следствие для нас: единственные способы знакомства, которые вообще не трогают n0, — явный адрес (ticket) и mDNS. DNS-дискавери по умолчанию — это звонок наружу, даже если оба узла рядом.

## 2. Отключение релея

Можно, явно в конфигурации эндпоинта:

```rust
let endpoint = Endpoint::builder(presets::Empty)
    .relay_mode(RelayMode::Disabled)
    .bind()
    .await?;
```

Источник: [Deploying to enterprise networks](https://docs.iroh.computer/configuring-networks) (раздел про полное отключение релея).

Документация прямо называет это «usually the wrong choice» и перечисляет, что ломается:

- «Roughly one connection in ten stops working» — для той десятой части сетевых условий, что сегодня спасает релей, никакого фоллбэка не остаётся, и это не диагностируется с сетевой стороны.
- «You lose the introduction, not just the fallback» — релей не только резервный путь, а механизм, которым два узла впервые находят друг друга и координируют hole punching. Без него узел за файрволом недостижим вообще, если нет другого способа знакомства.
- «Connections take longer to start» — без релея нет немедленного потока данных пока идёт попытка прямого соединения.
- «Network changes turn into outages» — переключение Wi-Fi↔мобильная сеть ломает прямой путь, и обычно релей мостит эти секунды перестройки; без релея — обрыв.

Отдельно: можно не отключать релей полностью, а подключить свой:

```rust
let relay: RelayUrl = "https://relay.internal.example.com".parse()?;
let endpoint = Endpoint::builder(presets::Empty)
    .relay_mode(RelayMode::Custom(RelayMap::from_iter([relay])))
    .bind()
    .await?;
```
Источник: тот же документ, раздел «Using your own relays» / self-hosted relay.

## 3. Пробивание NAT без релея

Релей у iroh участвует двояко: (а) как точка первого знакомства (introduction) и обмена кандидатами адресов, (б) как способ узнать свой публичный (reflexive) адрес — аналог STUN, через UDP 7842.

Факт (таблица типов NAT из инструмента диагностики):

| Тип NAT | Что это значит | Эффект |
|---|---|---|
| No NAT | локальный адрес = публичный | «Direct connections work with correct firewall config» |
| Endpoint-Independent | один исходящий UDP-пакет открывает порт для любого отправителя | «NAT traversal works reliably» |
| Endpoint-Dependent (симметричный) | отвечать может только тот адрес, куда слали | «Connections will primarily use relays» |

Источник: [Diagnose a connectivity issue](https://docs.iroh.computer/iroh-services/net-diagnostics/quickstart).

Из этого и из «Connections by explicit address still work» следует: без релея прямое соединение возможно **только если адрес уже известен и реально достижим** — то есть один из случаев:
- у узла действительно публичный IP (No NAT);
- у узла порт проброшен вручную на роутере;
- у узла порт открыт автоматически через UPnP/NAT-PMP — iroh по умолчанию сам просит у шлюза внешний порт этим способом («by default iroh asks your gateway for an external port over UPnP or NAT-PMP, which you can switch off», [Deploying to enterprise networks](https://docs.iroh.computer/configuring-networks)), и этот путь можно отключить (`PortmapperConfig::Disabled`).

В любом из этих случаев полученный адрес всё равно нужно передать второй стороне чем-то — тикетом, явным `EndpointAddr`, mDNS. Без релея и без одного из этих способов знакомства узел за обычным (не No-NAT) NAT просто недостижим — динамического обмена кандидатами, который делает релей, без релея нет.

Не нашёл: официального примера связки «UPnP + явный тикет» как готового рецепта без релея — только то, что каждая часть по отдельности документирована.

## 4. UDP заблокирован (разрешены только 80/443 по TCP)

Факт: прямое соединение в iroh — это всегда UDP. «Direct connections. Once introduced, the two devices try to connect straight to each other over UDP» ([Deploying to enterprise networks](https://docs.iroh.computer/configuring-networks)). TCP-пути для *прямого* P2P-соединения в iroh не существует — не нашёл ничего, что бы это опровергало.

Факт: сам релей умеет ходить по TCP. Соединение с релеем начинается как обычный HTTPS-запрос на 443 и затем апгрейдится до WebSocket («The relay connection itself... HTTPS, upgraded to WebSocket», там же). Это и есть путь, который переживает блокировку UDP на периметре — подтверждено отдельным тест-кейсом в самом iroh: [«test: UDP blocking test case» #4405](https://github.com/n0-computer/iroh/issues/4405) — «make sure that even when a firewall blocks all UDP traffic, two clients can still communicate via a relay» (закрыт PR [#4434](https://github.com/n0-computer/iroh/pull/4434)).

Важная оговорка из той же страницы: «If you run an application-aware firewall, a web filter, or a proxy that permits ordinary web browsing on port 443 but blocks WebSocket upgrades, iroh will not connect at all» — чистого TCP/443 недостаточно, нужен конкретно проход WebSocket-апгрейда на хосты релея.

Следствие: при заблокированном UDP между разными сетями **без релея соединения не будет вообще** — ни прямого (UDP запрещён), ни какого-либо второго relay-free пути (его нет в принципе, только relay-over-WebSocket). Релей здесь не «опция для скорости», а единственный канал.

Практический нюанс, который документация не покрывает, а issue — да: настройка `proxy_url` в iroh покрывает только relay-соединение, но не HTTPS-запросы Pkarr-дискавери (DNS-обнаружение) — «If you set proxy_url, the iroh relay connection honours it, but pkarr https lookup and publish does not... will make it just plain not work in situations where proxy_url is needed to get out at all» — [PR #4595 «send pkarr requests through the endpoint's proxy»](https://github.com/n0-computer/iroh/pull/4595), фиксит [issue #4478]. То есть в сети, где исходящий трафик обязан идти через HTTP-прокси (а не просто разрешён на 443), даже рабочий релей не спасёт DNS-дискавери — нужно либо чинить/обновляться, либо не завязываться на DNS-дискавери и знакомиться статическим адресом/mDNS.

Внутри самой сети UDP-блокировка периметра не при делах: «Two devices on the same office network connect directly to each other regardless of whether your perimeter firewall allows UDP. That traffic is governed by your internal network policy» (там же) — см. пункт 5.

## 5. Обе машины в одной локальной сети

Факт: если mDNS-крейт подключён, узнавание и соединение полностью локальны — релей не нужен ни для чего: «Fully airgapped. With no internet connection at all, devices can find each other by multicast on the local network and connect directly, with no relay involved» ([Deploying to enterprise networks](https://docs.iroh.computer/configuring-networks)); требование — свитчи пропускают multicast и выключена client isolation.

Но по умолчанию (`presets::N0`) включён не mDNS, а DNS/Pkarr-дискавери на `dns.iroh.link` — то есть «из коробки» даже два узла в одной комнате сходят в интернет к n0, если приложение не сконфигурировано на mDNS/`presets::Empty` явно. «Режим только локально» существует, но это осознанная сборка: `presets::Empty` + `RelayMode::Disabled` + добавленный mDNS-крейт.

Практический нюанс (issue #4623, ЗАКРЫТ 29.10.2026 как completed — см. поправку в `2026-11-02-own-relay-on-a-prod-box.md`: это была просьба о функции, а не баг, и ответ мейнтейнера «There is currently no way to perform the holepunching in iroh without having a relay transport» закрывает саму идею релея только для знакомства): даже когда прямое/локальное соединение не требует релея, `Endpoint::online()` по умолчанию ждёт именно релейное соединение. В сценарии с «только дискавери, без релея» (`enable_relay=false`, но адресный дискавери включён) эндпоинт продолжает каждые 16–32 секунды безрезультатно долбиться на WebSocket релея («idle endpoint makes a failed TCP connect every 16 to 32 s... `Failed to connect to relay server: unable to connect: Connection refused»), и `online()` никогда не становится true, хотя сами соединения этим не блокируются. Источник: [«a relay map entry used for address discovery only» #4623](https://github.com/n0-computer/iroh/issues/4623). Это шероховатость текущей реализации (1.3.0), не принципиальное ограничение, но учесть при проверке готовности эндпоинта в коде.

Смежная находка: у `iroh-relay` есть лёгкий режим «только обнаружение адреса» без ретрансляции трафика — QAD (QUIC Addr Discovery), флаг `enable_quic_addr_discovery` при `enable_relay=false`. Дешевле в хостинге, чем полный релей. Но прямо сейчас он не стартует без `--dev` при «боевом» TLS-сертификате — открытый баг: [«iroh-relay: QAD with the relay disabled fails to start unless --dev» #4622](https://github.com/n0-computer/iroh/issues/4622). Если рассматривать свой лёгкий «только для знакомства» сервер вместо полного релея — пока не готово для продакшена.

## Что не проверено

- Ничего не проверялось на реальном стенде/коде — только документация и issues. Поведение `RelayMode::Disabled` + mDNS в связке с нашим сценарием (бухгалтер↔сервер 1С) не воспроизводилось.
- Не нашёл официального рецепта «UPnP/NAT-PMP + явный тикет» как готовой схемы без релея — только то, что каждый элемент задокументирован отдельно.
- Не проверял, насколько типичные корпоративные firewalls/роутеры в принципе разрешают UPnP/NAT-PMP (по опыту вообще — обычно отключено из соображений безопасности), это общеизвестное ограничение, не специфичное для iroh и не подтверждённое фактом из документации iroh.
- QAD-only сервер (п. 5) — открытый баг на момент 2026-11-01, статус патча не проверялся.
- PR #4595 (proxy_url и pkarr) — не проверено, вошёл ли фикс в выпущенную версию крейта или ещё не слит.
- Не нашёл отдельного issue/обсуждения именно со словами «corporate network» от самих разработчиков n0 с их прямым ответом — ближайшее по духу — тест-кейс #4405 (их собственная инициатива) и страница `/configuring-networks`, которая написана именно для IT/security-отдела корпоративной сети.

## Таблица: случай → нужен ли релей → что будет без него

| Случай | Нужен ли релей | Что будет без релея |
|---|---|---|
| Обе машины в одной локальной сети | Нет, если явно включить mDNS-дискавери (`iroh-mdns-*`) и `RelayMode::Disabled`/`presets::Empty`. По умолчанию (`presets::N0`) — формально не обязателен для самого соединения, но узел всё равно пытается сходить на DNS-дискавери n0 и держит релейный коннект для `online()`. | Соединение работает полностью локально (multicast). Из шероховатостей — лишние неуспешные попытки дозвона до релея и повисший `online()==false`, если DNS/релей отключить не до конца. |
| Разные сети, UDP разрешён | Нет для самого канала данных *после* знакомства, но нужен для знакомства/hole punching, если адрес не передан статически. Без релея нужен внешний способ обмена адресами (ticket/EndpointAddr) и реально достижимый адрес (публичный IP, порт форвард, UPnP/NAT-PMP). | С релеем — работает «из коробки» в ~9 из 10 сетевых условий, автоматический hole punching. Без релея — работает только если адрес одной стороны заранее известен и достижим; иначе узел за NAT недостижим вовсе. |
| Разные сети, UDP заблокирован | Да, обязателен — и именно релей по TCP/WebSocket. | Без релея соединения не будет: прямого пути (всегда UDP) нет, альтернативного TCP P2P-пути в iroh не существует. Если вдобавок исходящий трафик обязан идти через прокси — проверить, что прокси покрывает и релей, и Pkarr-дискавери (известный пробел, PR #4595). |
| Одна сторона с публичным адресом и проброшенным портом | Нет для установления соединения (адрес реально достижим), но всё ещё нужен способ передать этот адрес второй стороне, если не используется mDNS/статический тикет. | Если адрес передан явно (ticket/EndpointAddr) — соединение прямое, релей не участвует. Если рассчитывать на динамический дискавери по умолчанию — без релея знакомство (кроме mDNS/статики) не произойдёт. |

## Что это значит для наших денег (дописано 2.11.2026 по вопросу владельца)

Владелец спросил: релей ведь только для знакомства, данные через него не идут, нагрузка копеечная? Ответ — наполовину.

- **Знакомство действительно копеечное.** Килобайты на соединение плюс удержание «онлайн»; один дешёвый узел держит много пар.
- **Но релей — ещё и запасной канал данных**, и в сети, где исходящий UDP закрыт (наружу только 80/443 TCP), через него идёт **весь** трафик по TCP/WebSocket. Прямой путь в iroh всегда UDP/QUIC, TCP-P2P-пути нет.
- **Эта «одна сеть из десяти» — наше целевое окружение.** Бухгалтер работает на удалённом сервере через RDP, корпоративная сеть, Касперский. Значит для нас доля релеированных соединений выше, чем общая статистика n0.
- **Трафик у нас не переписка, а папка.** Синхронизация выгрузки конфигурации — гигабайты на первой выгрузке ERP. Полоса платная и чужая.
- **Откупиться лёгким режимом нельзя:** QAD («только обнаружение адреса», `enable_quic_addr_discovery` при `enable_relay=false`) дешевле в хостинге, но помогает лишь там, где UDP и так проходит, и пока не стартует с боевым TLS ([#4622](https://github.com/n0-computer/iroh/issues/4622)).

Следствия, которые надо заложить в архитектуру, а не в надежду:

1. **Показывать, прямое соединение или через релей** — иначе мы не узнаем, сколько платим и за кого.
2. **Предел бесплатного — по подключениям/пресетам** (замысел владельца) и есть рычаг на чужую полосу: ограничивается не функция, а трафик через наш узел.
3. **Крупную первую выгрузку не гнать через релей молча**: либо предупреждать словами, либо давать обойти её тем путём, который у человека уже есть (RDP, общая папка).

## Ключевые ссылки

- [Deploying to enterprise networks](https://docs.iroh.computer/configuring-networks) — главный источник по UDP/TCP/релею/отключению частей.
- [NAT Traversal](https://docs.iroh.computer/concepts/nat-traversal) / [holepunching](https://www.iroh.computer/docs/protocols/net/holepunching)
- [mDNS / local address lookup](https://docs.iroh.computer/connecting/local-address-lookup)
- [Dial by NodeID, no address required!](https://www.iroh.computer/blog/iroh-dns) — DNS/Pkarr-дискавери
- [FAQ](https://docs.iroh.computer/about/faq)
- [Diagnose a connectivity issue](https://docs.iroh.computer/iroh-services/net-diagnostics/quickstart) — таблица типов NAT
- GitHub issues: [#4405](https://github.com/n0-computer/iroh/issues/4405) (тест блокировки UDP), [PR #4595](https://github.com/n0-computer/iroh/pull/4595) (proxy_url и pkarr), [#4623](https://github.com/n0-computer/iroh/issues/4623) и [#4622](https://github.com/n0-computer/iroh/issues/4622) (QAD-only релей)
