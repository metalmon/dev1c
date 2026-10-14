#!/usr/bin/env python3
"""Normalise SKILL.md files before they go into the image.

Skills arrive from Windows repositories, and two things break them silently:

1. A BOM (EF BB BF) before `---`. The parser does not see the opening delimiter
   and concludes there is no frontmatter — the skill is not loaded at all.
2. Unquoted values starting with `[`. YAML reads that as the start of a list, and
   `argument-hint: [-File <json> | -Value <str>]` destroys the parse of the whole
   block.

Both fixes are mechanical and repeatable on any fresh upstream version.
"""
import io
import os
import sys

try:
    import yaml
except ImportError:
    yaml = None

# Keys whose values may start with a YAML special character
RISKY_KEYS = ("argument-hint", "arguments", "usage", "example")


def normalize(path):
    raw = open(path, "rb").read()
    changed = []

    if raw.startswith(b"\xef\xbb\xbf"):
        raw = raw[3:]
        changed.append("BOM")

    text = raw.decode("utf-8", errors="replace")

    if "\r\n" in text:
        text = text.replace("\r\n", "\n")
        changed.append("CRLF")

    if text.startswith("---"):
        end = text.find("\n---", 3)
        if end > 0:
            fm, rest = text[3:end], text[end:]
            lines = []
            for line in fm.split("\n"):
                key = line.split(":", 1)[0].strip()
                if key in RISKY_KEYS and ":" in line:
                    k, v = line.split(":", 1)
                    v = v.strip()
                    # Quote only what YAML would otherwise read as structure
                    if v[:1] in "[{*&!%@`" and not v.startswith('"'):
                        v = '"' + v.replace('"', '\\"') + '"'
                        line = f"{k}: {v}"
                        changed.append(f"quoted {key}")
                lines.append(line)
            text = "---" + "\n".join(lines) + rest

    if changed:
        with io.open(path, "w", encoding="utf-8", newline="\n") as f:
            f.write(text)
    return changed


def main(root):
    total, fixed, broken = 0, 0, []
    for name in sorted(os.listdir(root)):
        path = os.path.join(root, name, "SKILL.md")
        if not os.path.isfile(path):
            continue
        total += 1
        changed = normalize(path)
        if changed:
            fixed += 1
            print(f"  {name}: {', '.join(changed)}")

        if yaml is None:
            continue
        text = open(path, encoding="utf-8").read()
        if not text.startswith("---"):
            broken.append((name, "no frontmatter"))
            continue
        end = text.find("\n---", 3)
        if end < 0:
            broken.append((name, "frontmatter not closed"))
            continue
        try:
            data = yaml.safe_load(text[3:end])
            if not isinstance(data, dict):
                broken.append((name, "frontmatter is not a mapping"))
            elif "name" not in data or "description" not in data:
                broken.append((name, "required fields name/description missing"))
        except Exception as exc:
            broken.append((name, f"YAML: {str(exc)[:80]}"))

    print(f"skills: {total}, files fixed: {fixed}")
    if broken:
        print("INVALID FRONTMATTER:", file=sys.stderr)
        for name, why in broken:
            print(f"  - {name}: {why}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "/etc/skel/.agents/skills"))
