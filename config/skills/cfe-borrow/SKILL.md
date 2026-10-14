---
name: cfe-borrow
description: >
  Этот скилл MUST быть вызван когда нужно заимствовать объект из конфигурации в расширение (CFE): перехватить метод, изменить форму, добавить реквизит.
  SHOULD также вызывать как первый шаг при создании patch-расширений.
  Do NOT использовать для создания расширения — используй cfe-init.
argument-hint: -ExtensionPath <path> -ConfigPath <path> -Object "Catalog.Контрагенты"
allowed-tools:
  - Bash
  - Read
  - Glob
---

# /cfe-borrow — Заимствование объектов из конфигурации

Заимствует объекты из основной конфигурации в расширение. Создаёт XML-файлы с `ObjectBelonging=Adopted` и `ExtendedConfigurationObject`, добавляет запись в ChildObjects расширения.

## Предусловие

Расширение должно быть создано (`/cfe-init`) и содержать валидный `Configuration.xml`.

## Параметры

| Параметр | Описание |
|----------|----------|
| `ExtensionPath` | Путь к каталогу расширения (обязат.) |
| `ConfigPath` | Путь к конфигурации-источнику (обязат.) |
| `Object` | Что заимствовать (обязат.), batch через `;;` |

## Формат -Object

- `Catalog.Контрагенты` — справочник
- `CommonModule.РаботаСФайлами` — общий модуль
- `Document.РеализацияТоваров` — документ
- `Enum.ВидыОплат` — перечисление
- `Catalog.X ;; CommonModule.Y ;; Enum.Z` — несколько объектов
Поддерживаются все 44 типа объектов конфигурации.

## Команда

```powershell
powershell.exe -NoProfile -File .claude/skills/cfe-borrow/scripts/cfe-borrow.ps1 -ExtensionPath src -ConfigPath C:\cfsrc\erp -Object "Catalog.Контрагенты"
```

## Примеры

```powershell
# Заимствовать один объект
... -ExtensionPath src -ConfigPath C:\cfsrc\erp -Object "Catalog.Контрагенты"

# Несколько объектов за раз
... -ExtensionPath src -ConfigPath C:\cfsrc\erp -Object "Catalog.Контрагенты ;; CommonModule.ОбщийМодуль ;; Enum.ВидыОплат"
```

## Верификация

```
/cfe-validate <ExtensionPath>
```

