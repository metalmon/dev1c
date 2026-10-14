---
name: cfe-init
description: >
  Этот скилл MUST быть вызван когда пользователь просит создать новое расширение конфигурации (CFE) для исправления, доработки или дополнения.
  SHOULD также вызывать когда нужно изменить типовую конфигурацию без прямых правок.
  Do NOT использовать для заимствования объектов — используй cfe-borrow.
argument-hint: <Name> [-Purpose Patch|Customization|AddOn] [-CompatibilityMode Version8_3_24]
allowed-tools:
  - Bash
  - Read
  - Glob
---

# /cfe-init — Создание расширения конфигурации 1С

Создаёт scaffold расширения: `Configuration.xml`, `Languages/Русский.xml`, опционально `Roles/`.

## Подготовка

Перед созданием расширения рекомендуется получить версию и режим совместимости базовой конфигурации:

```
/cf-info <ConfigPath> -Mode brief
```

Это даст `CompatibilityMode` (передать в `-CompatibilityMode`) и версию конфигурации (для `-Version`, например `<ВерсияКонфигурации>.1`).

## Параметры

| Параметр | Описание | По умолчанию |
|----------|----------|--------------|
| `Name` | Имя расширения (обязат.) | — |
| `Synonym` | Синоним | = Name |
| `NamePrefix` | Префикс собственных объектов | = Name + "_" |
| `OutputDir` | Каталог для создания | `src` |
| `Purpose` | `Patch` (исправление) / `Customization` (доработка) / `AddOn` (дополнение) | `Customization` |
| `Version` | Версия расширения | — |
| `Vendor` | Поставщик | — |
| `CompatibilityMode` | Режим совместимости | `Version8_3_24` |
| `NoRole` | Без основной роли | false |

## Команда

```powershell
powershell.exe -NoProfile -File .claude/skills/cfe-init/scripts/cfe-init.ps1 -Name "МоёРасширение"
```

## Что создаётся

```
<OutputDir>/
├── Configuration.xml         # Свойства расширения
├── Languages/
│   └── Русский.xml           # Язык (заимствованный)
└── Roles/                    # Если не -NoRole
    └── <Prefix>ОсновнаяРоль.xml
```

## Примеры

```powershell
# Расширение-исправление для ERP
... -Name Расш1 -Purpose Patch -CompatibilityMode Version8_3_17 -OutputDir src

# Расширение-доработка с версией
... -Name МоёРасширение -Version "1.0.0.1" -Vendor "Компания" -OutputDir src

# Без роли, с явным префиксом
... -Name ИсправлениеБага -NamePrefix "ИБ_" -Purpose Patch -NoRole -OutputDir src
```

## Верификация

```
/cfe-validate <OutputDir>
```

