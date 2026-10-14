---
name: inspect
description: >
  Этот скилл MUST быть вызван для анализа и инспекции любого объекта 1С из XML-выгрузки:
  конфигурация (cf-info), объект метаданных (meta-info), управляемая форма (form-info),
  схема компоновки данных СКД (skd-info), макет табличного документа MXL (mxl-info),
  подсистема (subsystem-info), роль и права (role-info).
  SHOULD вызывать как подготовительный шаг перед любой модификацией объекта.
  Do NOT использовать для модификации — для изменений используй соответствующие edit/compile скиллы.
argument-hint: >
  Конфигурация: <ConfigPath> [-Mode overview|brief|full]
  Метаданные: <ObjectPath> [-Mode overview|brief|full] [-Name <элемент>]
  Форма: <FormPath> [-Limit N] [-Offset N]
  СКД: <TemplatePath> [-Mode overview|query|fields|links|calculated|resources|params|variant|templates|trace|full] [-Name <набор|вариант|поле>]
  MXL: <TemplatePath> [-WithText] [-Format json]
  Подсистема: <SubsystemPath> [-Mode overview|content|ci|tree|full] [-Name <элемент>]
  Роль: <RightsPath> [-ShowDenied]
allowed-tools:
  - Bash
  - Read
  - Glob
---

# /inspect — Инспекция объектов 1С

Единый скилл для анализа и инспекции любого объекта 1С из XML-выгрузки. Заменяет отдельные cf-info, meta-info, form-info, skd-info, mxl-info, subsystem-info, role-info.

---

## 1. Конфигурация (Configuration.xml)

Читает `Configuration.xml` из выгрузки конфигурации и выводит компактное описание структуры.

### Параметры

| Параметр | Описание |
|----------|----------|
| `ConfigPath` | Путь к `Configuration.xml` или каталогу выгрузки |
| `Mode` | `overview` (default), `brief`, `full` |
| `Limit` / `Offset` | Пагинация (по умолчанию 150 строк) |
| `OutFile` | Записать результат в файл (UTF-8 BOM) |

```powershell
powershell.exe -NoProfile -File .claude/skills/inspect/scripts/cf-info.ps1 -ConfigPath "<путь>"
```

### Режимы

| Режим | Что показывает |
|---|---|
| `overview` *(default)* | Заголовок + ключевые свойства + таблица счётчиков объектов по типам |
| `brief` | Одна строка: Имя — "Синоним" vВерсия \| N объектов \| совместимость |
| `full` | Все свойства по категориям + полный список ChildObjects + DefaultRoles + мобильные функциональности |

### Примеры

```powershell
# Обзор конфигурации
... -ConfigPath upload/cfempty

# Краткая сводка
... -ConfigPath upload/acc_8.3.24 -Mode brief

# Полная информация с пагинацией
... -ConfigPath upload/acc_8.3.24 -Mode full -Limit 50 -Offset 100
```

---

## 2. Объект метаданных (XML-выгрузка)

Читает XML объекта метаданных и выводит компактное описание: реквизиты, табличные части, формы, движения, типы.

### Параметры

| Параметр | Описание |
|----------|----------|
| `ObjectPath` | Путь к XML-файлу объекта или каталогу (авто-резолв `<name>/<name>.xml`) |
| `Mode` | `overview` (default), `brief`, `full` |
| `Name` | Drill-down по имени элемента (реквизит, ТЧ, значение перечисления, шаблон URL, операция) |
| `Limit` / `Offset` | Пагинация (по умолчанию 150 строк) |
| `OutFile` | Записать результат в файл (UTF-8 BOM) |

```powershell
powershell.exe -NoProfile -File .claude/skills/inspect/scripts/meta-info.ps1 -ObjectPath "<путь>"
```

### Режимы

| Режим | Что показывает |
|---|---|
| `overview` *(default)* | Заголовок + ключевые свойства + структура без раскрытия деталей |
| `brief` | Всё одной-двумя строками: имена полей, счётчики |
| `full` | Всё раскрыто: колонки ТЧ, список источников подписки, движения, формы |

`-Name` — drill-down: раскрыть конкретный элемент (ТЧ, реквизит, шаблон URL, операцию веб-сервиса).

### Поддерживаемые типы (23)

**Ссылочные:** Справочник, Документ, Перечисление, Бизнес-процесс, Задача, План обмена, План счетов, ПВХ, ПВР
**Регистры:** Регистр сведений, Регистр накопления, Регистр бухгалтерии, Регистр расчёта
**Сервисные:** Отчёт, Обработка, HTTP-сервис, Веб-сервис, Общий модуль, Регламентное задание, Подписка на событие
**Прочие:** Константа, Журнал документов, Определяемый тип

### Примеры

```powershell
# Справочник — overview
... -ObjectPath Catalogs/Валюты/Валюты.xml

# Документ — полная сводка с колонками ТЧ, движениями, формами
... -ObjectPath Documents/АвансовыйОтчет/АвансовыйОтчет.xml -Mode full

# Регистр сведений — краткая сводка
... -ObjectPath InformationRegisters/КурсыВалют/КурсыВалют.xml -Mode brief

# Drill-down в ТЧ документа
... -ObjectPath Documents/АвансовыйОтчет/АвансовыйОтчет.xml -Name Товары

# HTTP-сервис — drill-down в шаблон URL
... -ObjectPath HTTPServices/ExternalAPI/ExternalAPI.xml -Name АктуальныеЗадачи

# Веб-сервис — drill-down в операцию
... -ObjectPath WebServices/EnterpriseDataUpload_1_0_1_1/EnterpriseDataUpload_1_0_1_1.xml -Name TestConnection
```

---

## 3. Управляемая форма (Form.xml)

Читает `Form.xml` и выводит компактную сводку: дерево элементов, реквизиты с типами, команды, события. Заменяет необходимость читать тысячи строк XML.

### Параметры

| Параметр | Обязательный | По умолчанию | Описание |
|-----------|:------------:|--------------|----------|
| `FormPath` | да | — | Путь к файлу `Form.xml` |
| `Limit` | нет | `150` | Макс. строк вывода |
| `Offset` | нет | `0` | Пропустить N строк (для пагинации) |

```powershell
powershell.exe -NoProfile -File .claude/skills/inspect/scripts/form-info.ps1 -FormPath "<путь к Form.xml>"
```

С пагинацией:
```powershell
... -FormPath "<путь>" -Offset 150
```

### Чтение вывода

**Elements** — компактное дерево UI-элементов с типами, привязками к данным, флагами и событиями:

```
Elements:
  ├─ [Group:AH] ГруппаШапка
  │  ├─ [Input] Организация -> Объект.Организация {OnChange}
  │  └─ [Input] Договор -> Объект.Договор [visible:false] {StartChoice}
  ├─ [Table] Товары -> Объект.Товары
  │  └─ [Input] Номенклатура -> Объект.Товары.Номенклатура {OnChange}
  └─ [Pages] Страницы
     ├─ [Page] Основное (5 items)
     └─ [Page] Печать (2 items)
```

Сокращения типов: `[Group:V/H/AH/AV]`, `[Input]`, `[Check]`, `[Label]`, `[Table]`, `[Button]`, `[Pages]`, `[Page]`, `[Popup]`
Флаги: `[visible:false]`, `[enabled:false]`, `[ro]`, `,collapse`

**Attributes** — реквизиты формы (тип, ValueTable с колонками, DynamicList → MainTable).
**Parameters** — параметры формы с ключевым параметром `(key)`.
**Commands** — `Имя -> Обработчик [Сочетание]`.
**Events** — обработчики событий формы.

---

## 4. СКД — Схема компоновки данных (Template.xml)

Читает `Template.xml` схемы компоновки данных (СКД/DCS) и выводит компактную сводку. Подробная справка по режимам — в `references/skd-modes-reference.md`.

### Параметры

| Параметр | Описание |
|----------|----------|
| `TemplatePath` | Путь к `Template.xml` или каталогу макета (авто-резолв в `Ext/Template.xml`) |
| `Mode` | Режим анализа (по умолчанию `overview`) |
| `Name` | Имя набора (query), поля (fields/calculated/resources/trace), варианта (variant), группировки/поля (templates) |
| `Batch` | Номер пакета запроса, 0 = все (только query) |
| `Limit` / `Offset` | Пагинация (по умолчанию 150 строк) |
| `OutFile` | Записать результат в файл (UTF-8 BOM) |

```powershell
powershell.exe -NoProfile -File .claude/skills/inspect/scripts/skd-info.ps1 -TemplatePath "<путь>"
```

С указанием режима:
```powershell
... -Mode query -Name НоменклатураСЦенами
... -Mode query -Name ДанныеТ13 -Batch 3
... -Mode fields -Name КадастроваяСтоимость
... -Mode trace -Name "Коэффициент Ки"
... -Mode variant -Name 1
... -Mode templates -Name ВидНалоговойБазы
```

### Режимы

| Режим | Без `-Name` | С `-Name` |
|-------|-------------|-----------|
| `overview` | Навигационная карта схемы + подсказки Next | — |
| `query` | — | Текст запроса набора (с оглавлением батчей) |
| `fields` | Карта: имена полей по наборам | Деталь поля: набор, тип, роль, формат |
| `links` | Все связи наборов | — |
| `calculated` | Карта: имена вычисляемых полей | Выражение + заголовок + ограничения |
| `resources` | Карта: имена ресурсов (`*` = групповые формулы) | Формулы агрегации по группировкам |
| `params` | Таблица параметров: тип, значение, видимость | — |
| `variant` | Список вариантов | Структура группировок + фильтры + вывод |
| `templates` | Карта привязок шаблонов (field/group) | Содержимое шаблона: строки, ячейки, выражения |
| `trace` | — | Полная цепочка: набор → вычисление → ресурс |
| `full` | Полная сводка: overview + query + fields + resources + params + variant | — |

### Типичный workflow

1. `overview` — понять структуру, увидеть подсказки
2. `trace -Name <поле>` — узнать как считается колонка отчёта (за один вызов)
3. `query -Name <набор>` — посмотреть текст SQL-запроса
4. `variant -Name <N>` — посмотреть группировки и фильтры варианта

---

## 5. MXL — Макет табличного документа (Template.xml)

Читает `Template.xml` табличного документа и выводит компактную сводку: именованные области, параметры, наборы колонок.

### Параметры

| Параметр | По умолчанию | Описание |
|----------|:------------:|----------|
| `TemplatePath` | — | Прямой путь к `Template.xml` |
| `ProcessorName` | — | Имя обработки (альтернатива пути) |
| `TemplateName` | — | Имя макета (альтернатива пути) |
| `SrcDir` | `src` | Каталог исходников |
| `Format` | `text` | Формат вывода: `text` или `json` |
| `WithText` | false | Включить статический текст и шаблоны |
| `MaxParams` | 10 | Макс. параметров в списке на область |
| `Limit` | 150 | Макс. строк вывода |
| `Offset` | 0 | Пропустить N строк (для пагинации) |

```powershell
powershell.exe -NoProfile -File .claude/skills/inspect/scripts/mxl-info.ps1 -TemplatePath "<путь>"
```

Или по имени обработки/макета:
```powershell
... -ProcessorName "<Имя>" -TemplateName "<Макет>" [-SrcDir "<каталог>"]
```

Дополнительные флаги:
```powershell
... -WithText              # включить текстовое содержимое ячеек
... -Format json           # JSON-вывод для программной обработки
... -MaxParams 20          # показать больше параметров на область
... -Offset 150            # пагинация
```

### Чтение вывода

**Named areas** — области перечислены в порядке документа (сверху вниз):
```
--- Named areas ---
  Заголовок          Rows     rows 1-4     (1 params)
  Строка             Rows     rows 14-14   (8 params)
  Итого              Rows     rows 16-17   (1 params)
```

Типы: **Rows** (горизонтальная), **Columns** (вертикальная), **Rectangle** (строки+колонки), **Drawing** (рисунок/штрихкод).

**Parameters by area** — параметры с `detailParameter` (расшифровка):
```
  Строка: НомерСтроки, Товар, Количество, Цена, Сумма, ... (+3)
    detail: Товар->Номенклатура
```

**Параметры `[tpl]`** — встроены в шаблонный текст: `"Инв № [ИнвентарныйНомер]"`, заполняются программно как обычные.

**Intersections** (при наличии Rows+Columns): `Макет.ПолучитьОбласть("ВысотаЭтикетки|ШиринаЭтикетки")`

---

## 6. Подсистема (XML-выгрузка)

Читает XML подсистемы и выводит компактное описание: состав, дочерние подсистемы, командный интерфейс, дерево иерархии.

### Параметры

| Параметр | Описание |
|----------|----------|
| `SubsystemPath` | Путь к XML-файлу подсистемы, каталогу подсистемы или каталогу `Subsystems/` (для tree) |
| `Mode` | `overview` (default), `content`, `ci`, `tree`, `full` |
| `Name` | Drill-down: тип объекта в content, секция в ci, имя подсистемы в tree |
| `Limit` / `Offset` | Пагинация (по умолчанию 150 строк) |
| `OutFile` | Записать результат в файл (UTF-8 BOM) |

```powershell
powershell.exe -NoProfile -File .claude/skills/inspect/scripts/subsystem-info.ps1 -SubsystemPath "<путь>"
```

### Режимы

| Режим | Что показывает |
|---|---|
| `overview` *(default)* | Компактная сводка: свойства, состав (сгруппирован по типам), дочерние подсистемы, наличие CI |
| `content` | Список Content с группировкой по типу. `-Name Catalog` — только каталоги |
| `ci` | Разбор `CommandInterface.xml`: видимость, размещение, порядок команд/подсистем/групп |
| `tree` | Рекурсивное дерево иерархии подсистем с маркерами [CI], [OneCmd], [Скрыт] |
| `full` | overview + content + ci в одном вызове |

### Примеры

```powershell
# Обзор подсистемы
... -SubsystemPath Subsystems/Продажи.xml

# Состав — только документы
... -SubsystemPath Subsystems/Продажи.xml -Mode content -Name Document

# Командный интерфейс
... -SubsystemPath Subsystems/Продажи.xml -Mode ci

# Дерево подсистем от корня
... -SubsystemPath Subsystems -Mode tree

# Дерево для одной подсистемы
... -SubsystemPath Subsystems -Mode tree -Name Администрирование
```

---

## 7. Роль и права (Rights.xml)

Парсит `Rights.xml` роли и выдаёт компактную сводку: объекты сгруппированы по типу, показаны только разрешённые права. Сжатие: тысячи строк XML → 50–150 строк текста.

### Параметры

| Параметр | Описание |
|----------|----------|
| `RightsPath` | Путь к файлу `Rights.xml` (обычно `Roles/ИмяРоли/Ext/Rights.xml`) |
| `ShowDenied` | Показать запрещённые права (по умолчанию скрыты) |
| `Limit` / `Offset` | Пагинация (по умолчанию 150 строк; `0` = без ограничений) |
| `OutFile` | Записать результат в файл (UTF-8 BOM) |

```powershell
powershell.exe -File .claude/skills/inspect/scripts/role-info.ps1 -RightsPath <path> -OutFile <output.txt>
```

**Важно:** Всегда используй `-OutFile` и читай результат через Read tool. Прямой вывод в консоль ломает кириллицу.

### Формат вывода

```
=== Role: БазовыеПраваБП --- "Базовые права: Бухгалтерия предприятия" ===

Allowed rights:

  Catalog (8):
    Контрагенты: Read, View, InputByString
    ...

  InformationRegister (6):
    ЦеныНоменклатуры: Read [RLS], Update
    ...

RLS: 4 restrictions
Templates: ДляРегистра, ПоЗначениям
Total: 138 allowed, 18 denied
```

- `[RLS]` — право с ограничением на уровне записей (restrictionByCondition)
- Вложенные объекты: `Контрагенты.StandardAttribute.PredefinedDataName`

---

## Общие правила пагинации

Все скрипты поддерживают `-Limit` (по умолчанию 150) и `-Offset`. При усечении:
```
[TRUNCATED] Shown 150 of 220 lines. Use -Offset 150 to continue.
```
