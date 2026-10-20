# Слой подсказок онбординга: план постройки

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** программа, которая при входе в сессию стенда проводит программиста 1С по шести шагам, рисуя поверх живого рабочего стола вырезанные по контуру выноски со стрелками.

**Architecture:** три чистых модуля без GTK (прицел, сценарий, размещение), поверх них окно на GTK3 с маской формы и пустой входной областью, и машина шагов, которая решает, что показывать. Чистые модули проверяются `unittest` где угодно, окно — снимком под Xvfb.

**Tech Stack:** Python 3, PyGObject (GTK 3), cairo. Тексты в `scenario.toml`. Запуск из `/etc/xdg/autostart` и из меню приложений.

**Spec:** `docs/superpowers/specs/2026-10-20-onboarding-overlay-design.md`

## Global Constraints

- Никакой полупрозрачности: формы режутся расширением SHAPE, композитор не требуется.
- Одна подсказка на экране одновременно.
- Мышь проходит везде, кроме самой выноски.
- Esc и крестик закрывают любой шаг, хотя в согласованных текстах кнопка [Закрыть] есть только у первого.
- Тексты шести шагов берутся из спецификации дословно; слово «git» в них не встречается ни разу.
- Показывается при каждом входе в сессию; отметка о закрытии живёт до конца сеанса, не в домашнем каталоге.
- Кнопки второго акта в первой версии нет: на шестом шаге только [Готово].
- Чистые модули (`targets`, `scenario`, `placement`, `steps`) не импортируют `gi` ни на каком уровне: иначе их нельзя проверить вне контейнера.
- Координаты считаются от геометрии экрана и окон; в пиксели ничего не зашивается, кроме калибровки сетки значков.
- Пакеты в образ: `python3-gi`, `python3-gi-cairo`, `gir1.2-gtk-3.0`.

## Review Focus

- **Экран другого разрешения.** Стенд пришёл с 2560×1440, наброски рисовались на 1280×800. Выноска обязана остаться целиком на экране при любом размере. Тест в задаче 3.
- **Устаревшие записи в раскладке xfdesktop.** Файл помнит значки, которых уже нет (`1c-enterprise.desktop`, `1c-configurator.desktop` у dev1), и две записи делят одну клетку. Прицел должен брать только существующий файл. Тест в задаче 1.
- **Предмет закрыт другим окном.** Набросок целился в значок верно, но его закрывал развёрнутый Zed. «Предмет на экране» значит «не закрыт», а не «существует». Тест в задаче 5.
- **Шаг пропущен человеком.** Кнопка «Дальше» доступна всегда, в том числе когда предмет так и не появился; машина шагов не должна застревать. Тест в задаче 5.
- **Zed не запущен в момент копирования вопроса.** Четвёртый шаг кладёт вопрос в буфер обмена; отсутствие Zed не должно ронять программу. Тест в задаче 6.

---

## File Structure

| Файл | Ответственность |
| --- | --- |
| `config/1c/onboarding/targets.py` | куда показывать: значок рабочего стола по раскладке xfdesktop, окно по классу или имени, область трея на панели |
| `config/1c/onboarding/scenario.py` | загрузка и проверка `scenario.toml` |
| `config/1c/onboarding/scenario.toml` | шесть шагов: тексты, цели, кнопки |
| `config/1c/onboarding/placement.py` | где встаёт выноска и откуда куда идёт стрелка |
| `config/1c/onboarding/steps.py` | машина шагов: что показано, когда уходит дальше |
| `config/1c/onboarding/callout.py` | окно GTK: рисование, маска формы, входная область |
| `config/1c/onboarding/main.py` | связывание всего, буфер обмена, запуск программ |
| `config/1c/onboarding/tests/test_*.py` | `unittest`, без GTK кроме задачи 4 |
| `config/1c/onboarding.desktop` | пункт меню «Знакомство со стендом» |
| `config/1c/onboarding-autostart.desktop` | запуск при входе в сессию |
| `Dockerfile.1c` | пакеты, копирование, автозапуск, пункт меню |
| `config/1c/stand.sh` | команда `1c-stand onboarding` |

Тесты всегда запускаются из каталога `config/1c/onboarding`:
`cd config/1c/onboarding && python3 -m unittest tests.test_targets -v`, и так же
для остальных. Задачи 1–3 и 5 проходят на любой машине с Python 3, задачи 4 и 6
— в контейнере.

---

### Task 1: Прицел

**Files:**
- Create: `config/1c/onboarding/targets.py`
- Create: `config/1c/onboarding/tests/test_targets.py`
- Create: `config/1c/onboarding/tests/fixtures/icons.screen0.yaml`

**Interfaces:**
- Produces: `Rect = namedtuple('Rect', 'x y w h')`; `desktop_icon(layout_text: str, desktop_file: str, exists: Callable[[str], bool], icon_size: int = 48) -> Rect | None`; `window(lister: Callable[[], list[WindowInfo]], name_pattern: str) -> Rect | None`; `tray_area(workarea: Rect) -> Rect`; `anchor_in(rect: Rect, anchor: str) -> Rect`; `WindowInfo = namedtuple('WindowInfo', 'name wclass rect stacking')`.

`anchor_in` сужает прямоугольник окна до его части: `'whole'`, `'right-bottom'`,
`'right'`. Нужен пятому шагу сценария, который показывает не на окно Zed целиком,
а на панель агента справа внизу — своих окон Zed не отдаёт, и взять эту область
можно только долей от окна.

- [ ] **Step 1: Напиши падающий тест**

Фикстура — копия настоящего файла dev1 (в нём есть устаревшие записи и две записи в одной клетке).

```python
def test_icon_of_existing_file():
    r = desktop_icon(FIXTURE, '1c-start.desktop', exists=lambda p: True)
    self.assertEqual((r.x + r.w // 2, r.y + r.h // 2), (60, 146))

def test_stale_entry_is_ignored():
    r = desktop_icon(FIXTURE, '1c-enterprise.desktop', exists=lambda p: False)
    self.assertIsNone(r)

def test_unknown_file():
    self.assertIsNone(desktop_icon(FIXTURE, 'нет-такого.desktop', exists=lambda p: True))
```

- [ ] **Step 2: Запусти тест, убедись, что падает**

Run: `python3 -m unittest tests.test_targets -v`
Expected: FAIL, `ModuleNotFoundError: targets`

- [ ] **Step 3: Реализуй `targets.py`**

Калибровка сетки: первый значок центром в (60, 40), шаг 106 по обеим осям при `icon_size` 48 — замерено по снимку рабочего стола стенда. Выдели её в константы модуля: это первое, что будут править, если стрелка промахнётся.

- [ ] **Step 4: Запусти тест, убедись, что проходит**

Run: `python3 -m unittest tests.test_targets -v`
Expected: PASS

- [ ] **Step 5: Коммит**

```bash
git add config/1c/onboarding/targets.py config/1c/onboarding/tests/
git commit -m "Онбординг: прицел по раскладке xfdesktop и по окнам"
```

---

### Task 2: Сценарий

**Files:**
- Create: `config/1c/onboarding/scenario.toml`
- Create: `config/1c/onboarding/scenario.py`
- Create: `config/1c/onboarding/tests/test_scenario.py`

**Interfaces:**
- Consumes: ничего.
- Produces: `Step` с полями `number, title, body, target, buttons, advance_on`; `load(path: str) -> list[Step]`; `TARGET_KINDS = ('none', 'desktop_icon', 'window', 'tray')`.

`scenario.toml` содержит шесть шагов дословно из раздела «Сценарий» спецификации. Цели: шаг 1 — `none`, шаги 2 и 3 — `desktop_icon` (`1c-start.desktop`, `dev.zed.Zed.desktop`), шаг 4 — `window` по Zed, шаг 5 — `window` по Zed, шаг 6 — `tray`.

- [ ] **Step 1: Напиши падающий тест**

```python
def test_six_steps_in_order():
    steps = load(SCENARIO)
    self.assertEqual([s.number for s in steps], [1, 2, 3, 4, 5, 6])

def test_texts_are_verbatim():
    steps = load(SCENARIO)
    self.assertEqual(steps[1].title, 'Сначала знакомое')
    self.assertIn('заведите одну номенклатуру', steps[1].body)

def test_no_git_word_anywhere():
    text = ' '.join(s.title + ' ' + s.body for s in load(SCENARIO)).lower()
    self.assertNotIn('git', text)

def test_unknown_target_kind_is_rejected():
    with self.assertRaises(ValueError):
        load(BROKEN_SCENARIO)   # цель kind = "луна"
```

- [ ] **Step 2: Запусти, убедись, что падает**

Run: `python3 -m unittest tests.test_scenario -v`
Expected: FAIL

- [ ] **Step 3: Реализуй `scenario.py` и напиши `scenario.toml`**

Разбор — `tomllib` из стандартной библиотеки (Python 3.11+ в образе Debian 13). Неизвестный `kind` и пропущенное обязательное поле поднимают `ValueError` с номером шага в сообщении.

- [ ] **Step 4: Запусти, убедись, что проходит**

Run: `python3 -m unittest tests.test_scenario -v`
Expected: PASS

- [ ] **Step 5: Коммит**

```bash
git add config/1c/onboarding/scenario.py config/1c/onboarding/scenario.toml config/1c/onboarding/tests/test_scenario.py
git commit -m "Онбординг: шесть шагов данными, а не кодом"
```

---

### Task 3: Размещение

**Files:**
- Create: `config/1c/onboarding/placement.py`
- Create: `config/1c/onboarding/tests/test_placement.py`

**Interfaces:**
- Consumes: `Rect` из `targets`.
- Produces: `Layout = namedtuple('Layout', 'bubble arrow_start arrow_end')`; `place(screen: Rect, target: Rect | None, bubble_size: tuple[int, int], margin: int = 24) -> Layout`; `bubble_size(step: Step, width: int = 640, line_height: int = 21, head: int = 64, foot: int = 24) -> tuple[int, int]`.

`bubble_size` считается здесь, а не в окне: иначе размер выноски зависел бы от
обмера текста в GTK, а размещение — от размера, и круг бы не разорвался. Ширина
задана числом, высота — от числа строк тела. Задача 4 рисует по тем же числам.

При `target is None` выноска ставится по центру экрана, `arrow_start == arrow_end`.

- [ ] **Step 1: Напиши падающий тест**

```python
SCREEN = Rect(0, 0, 2560, 1440)

def test_bubble_never_leaves_the_screen():
    for target in (Rect(36, 122, 48, 48), Rect(2500, 1400, 24, 24), Rect(1280, 0, 48, 48)):
        b = place(SCREEN, target, (640, 150)).bubble
        self.assertGreaterEqual(b.x, 0)
        self.assertGreaterEqual(b.y, 0)
        self.assertLessEqual(b.x + b.w, SCREEN.w)
        self.assertLessEqual(b.y + b.h, SCREEN.h)

def test_bubble_never_covers_its_target():
    target = Rect(36, 122, 48, 48)
    b = place(SCREEN, target, (640, 150)).bubble
    self.assertFalse(overlaps(b, target))

def test_small_screen_still_fits():
    b = place(Rect(0, 0, 1280, 800), Rect(36, 330, 48, 48), (640, 150)).bubble
    self.assertLessEqual(b.x + b.w, 1280)

def test_no_target_centres_the_bubble():
    layout = place(SCREEN, None, (640, 150))
    self.assertEqual(layout.arrow_start, layout.arrow_end)
```

- [ ] **Step 2: Запусти, убедись, что падает**

Run: `python3 -m unittest tests.test_placement -v`
Expected: FAIL

- [ ] **Step 3: Реализуй `place` в `placement.py`**

Порядок предпочтений: справа от цели, слева, снизу, сверху; первый вариант, который целиком влезает в экран и не пересекает цель. Если не влез ни один, выноска прижимается к краю, противоположному цели.

- [ ] **Step 4: Запусти, убедись, что проходит**

Run: `python3 -m unittest tests.test_placement -v`
Expected: PASS

- [ ] **Step 5: Коммит**

```bash
git add config/1c/onboarding/placement.py config/1c/onboarding/tests/test_placement.py
git commit -m "Онбординг: выноска не уезжает за край и не закрывает цель"
```

---

### Task 4: Окно выноски

**Files:**
- Create: `config/1c/onboarding/callout.py`
- Create: `config/1c/onboarding/tests/test_callout_shape.py`

**Interfaces:**
- Consumes: `Layout` из `placement`, `Step` из `scenario`.
- Produces: класс `Callout(step, layout, on_button: Callable[[str], None])` с методами `show()`, `hide()`; свойство `window` для тестов.

- [ ] **Step 1: Напиши падающий тест**

Снимок под Xvfb: корень заливается известным цветом, поверх показывается выноска, затем проверяется пиксель в прямоугольнике окна, но вне фигуры.

```python
def test_window_is_shaped_not_rectangular():
    # Xvfb :77, корень залит синим
    callout = Callout(step, layout, on_button=lambda _: None)
    callout.show()
    pump_gtk(1.0)
    img = grab(':77')
    self.assertEqual(img.pixel(layout.bubble.x - 10, layout.bubble.y - 10), BLUE)
    self.assertEqual(img.pixel(layout.bubble.x + 20, layout.bubble.y + 20), WHITE)
```

- [ ] **Step 2: Запусти в контейнере, убедись, что падает**

Run: `docker exec dev1c python3 -m unittest tests.test_callout_shape -v`
Expected: FAIL

- [ ] **Step 3: Реализуй `Callout` в `callout.py`**

`Gtk.WindowType.POPUP`, `set_app_paintable(True)`, `set_keep_above(True)`, `set_accept_focus(False)`. Фигура: та же функция рисования выполняется на `cairo.ImageSurface`, из непрозрачных пикселей строится область через `Gdk.cairo_region_create_from_surface`, она отдаётся в `shape_combine_region`. Входная область — только прямоугольник выноски, тем же способом, через `input_shape_combine_region`. Esc и крестик зовут `on_button('close')`.

- [ ] **Step 4: Запусти, убедись, что проходит**

Run: `docker exec dev1c python3 -m unittest tests.test_callout_shape -v`
Expected: PASS

- [ ] **Step 5: Коммит**

```bash
git add config/1c/onboarding/callout.py config/1c/onboarding/tests/test_callout_shape.py
git commit -m "Онбординг: окно, вырезанное по контуру, с прозрачной для мыши стрелкой"
```

---

### Task 5: Машина шагов

**Files:**
- Create: `config/1c/onboarding/steps.py`
- Create: `config/1c/onboarding/tests/test_steps.py`

**Interfaces:**
- Consumes: `Step` из `scenario`, `Rect`/`WindowInfo` из `targets`.
- Produces: `Machine(steps, world)` с `current -> Step`, `visible_target() -> Rect | None`, `tick()`, `press(button: str)`, `done -> bool`. `world` — объект с методами `icon(desktop_file)`, `window(pattern)`, `covered(rect) -> bool`.

- [ ] **Step 1: Напиши падающий тест**

```python
def test_step_advances_when_its_window_appears():
    m = Machine(steps, world=FakeWorld(windows=[]))
    m.press('next')            # перешли на шаг 2, цель — значок 1С
    world.windows = [WindowInfo('Управление нашей фирмой (учебная)', '1cv8', R, 0)]
    m.tick()
    self.assertEqual(m.current.number, 3)

def test_arrow_hidden_while_target_is_covered():
    m = Machine(steps, world=FakeWorld(covered=True))
    m.press('next')
    self.assertIsNone(m.visible_target())

def test_next_always_works_even_without_the_target():
    m = Machine(steps, world=FakeWorld(windows=[]))
    for _ in range(6):
        m.press('next')
    self.assertTrue(m.done)

def test_close_ends_everything():
    m = Machine(steps, world=FakeWorld())
    m.press('close')
    self.assertTrue(m.done)
```

- [ ] **Step 2: Запусти, убедись, что падает**

Run: `python3 -m unittest tests.test_steps -v`
Expected: FAIL

- [ ] **Step 3: Реализуй `Machine` в `steps.py`**

`visible_target` возвращает `None`, если цели нет на экране или она закрыта другим окном: стрелка тогда не рисуется, выноска остаётся. `tick` вызывается по таймеру и двигает шаг вперёд, если выполнено условие `advance_on` этого шага.

- [ ] **Step 4: Запусти, убедись, что проходит**

Run: `python3 -m unittest tests.test_steps -v`
Expected: PASS

- [ ] **Step 5: Коммит**

```bash
git add config/1c/onboarding/steps.py config/1c/onboarding/tests/test_steps.py
git commit -m "Онбординг: шаг уходит вперёд по факту, а стрелка ждёт свой предмет"
```

---

### Task 6: Программа целиком и место в образе

**Files:**
- Create: `config/1c/onboarding/world.py`
- Create: `config/1c/onboarding/main.py`
- Create: `config/1c/onboarding.desktop`
- Create: `config/1c/onboarding-autostart.desktop`
- Create: `config/1c/onboarding/tests/test_clipboard.py`
- Modify: `Dockerfile.1c` (пакеты в стадию `tools`, копирование и пункты меню в стадию `stand`)
- Modify: `config/1c/stand.sh` (команда `onboarding`, запуск из `seed` не трогаем)
- Modify: `config/1c/test-stand.sh` (проверка, что файлы на месте)

**Interfaces:**
- Consumes: всё предыдущее.
- Produces: `World` в `world.py` с методами `icon(desktop_file) -> Rect | None`, `window(pattern, anchor='whole') -> Rect | None`, `covered(rect) -> bool`, `screen() -> Rect` — настоящая реализация того мира, который задача 5 подделывала; строится поверх чистых функций `targets` и запросов к `Gdk.Screen`. `main.run(scenario_path: str) -> int`; `copy_question(text: str) -> bool`.

- [ ] **Step 1: Напиши падающий тест**

```python
def test_copy_question_survives_absent_display():
    # DISPLAY пуст: буфера нет, но падать нельзя
    self.assertFalse(copy_question('Какая номенклатура есть в базе'))
```

- [ ] **Step 2: Запусти, убедись, что падает**

Run: `docker exec dev1c python3 -m unittest tests.test_clipboard -v`
Expected: FAIL

- [ ] **Step 3: Реализуй `main.py` и файлы запуска**

`copy_question` кладёт текст в `Gtk.Clipboard` и возвращает `False`, если дисплея нет. Кнопки шагов 2 и 3 запускают `1c-stand start1c` и `zed` через `subprocess.Popen`, не дожидаясь их. Отметка о закрытии — файл в `$XDG_RUNTIME_DIR`, то есть живёт до конца сеанса.

В образе программа лежит в `/opt/onboarding`, точка входа зовётся `1c-stand onboarding`, и обе записи `.desktop` вызывают именно её, а не `python3` напрямую: так путь к интерпретатору и сценарию остаётся в одном месте.

- [ ] **Step 4: Собери образ и проверь на живом стенде**

Run: `./provision.sh build && ./provision.sh run …`, затем войти по RDP.
Expected: при входе появляется первый шаг; шаги 2 и 3 доводят до открытых 1С и Zed; мышь проходит мимо выносок; `docker exec dev1c test-stand` зелёный.

- [ ] **Step 5: Коммит**

```bash
git add config/1c/onboarding/main.py config/1c/onboarding*.desktop Dockerfile.1c config/1c/stand.sh config/1c/test-stand.sh config/1c/onboarding/tests/test_clipboard.py
git commit -m "Онбординг: запуск при входе в сессию и место в образе"
```

---

## За границей этого плана

**Второй акт** (шаги 7–13 спецификации) — отдельный план. Он опирается на тот
же движок, но ждёт двух вещей: прогона круга с настоящей правкой и решения,
чем набит `~/project` на свежем стенде. Делать его до этого нельзя по условию
выпуска, записанному в спецификации.

**Три посторонние поломки**, всплывшие при проектировании, чинятся независимо
и в этот план не входят: правила агента на стенде описывают туннель, которого
там нет; `keymap.json` для Zed в образе не ставится вовсе; снимки для финальных
экранов нужны чистые.
