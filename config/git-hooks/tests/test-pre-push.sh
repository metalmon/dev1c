#!/bin/sh
# Проверка хука pre-push: он обязан не пустить в Gitea модуль, который не
# разбирается, и обязан не мешать во всех остальных случаях.
#
# Анализатор подменяется заглушкой в PATH: настоящий живёт в образе, а
# проверять надо поведение хука, а не его. Прогон: sh config/git-hooks/tests/test-pre-push.sh
set -u
HOOK=$(cd "$(dirname "$0")/.." && pwd)/pre-push
fails=0
say() { printf '%s\n' "$*"; }

# Репозиторий с одним коммитом в прошлом и одним «отправляемым».
# Печатает: <база> <вершина>. Зовётся в подстановке, то есть в подоболочке,
# поэтому её `cd` до теста не доезжает — каталог меняет сам тест, строкой
# ниже. На этом тест однажды уже обманулся: git в хуке смотрел не на тот
# репозиторий, ревизий не находил и честно отвечал «менять нечего».
make_repo() {
    d=$1; file=$2
    mkdir -p "$d" && cd "$d" || exit 1
    git init -q . && git config user.email t@t && git config user.name t
    # hooksPath образа в тестовом репозитории не нужен: хук зовём руками.
    git config core.hooksPath ''
    mkdir -p src/Catalogs
    echo 'читай' > README.md
    git add -A && git commit -qm base
    base=$(git rev-parse HEAD)
    mkdir -p "$(dirname "$file")"
    echo 'Процедура П() КонецПроцедуры' > "$file"
    git add -A && git commit -qm change
    printf '%s %s\n' "$base" "$(git rev-parse HEAD)"
}

# Заглушка анализатора: печатает в stdout то, что ей велели, и выходит с
# кодом 0 — настоящий тоже не обязан падать, когда нашёл замечания.
fake_analyzer() {
    dir=$1; out=$2
    mkdir -p "$dir"
    cat > "$dir/bsl-analyzer" <<EOF
#!/bin/sh
printf '%s\n' '$out'
EOF
    chmod +x "$dir/bsl-analyzer"
}

run_hook() { # $1 = base, $2 = tip; stdin хука — строка pre-push
    printf 'refs/heads/dev1 %s refs/heads/dev1 %s\n' "$2" "$1" | sh "$HOOK" origin http://localhost:3000/dev1c/x.git
}

check() { # $1 = что проверяли, $2 = ожидаемый код, $3 = полученный
    if [ "$2" = "$3" ]; then say "ok   $1"; else say "FAIL $1: ждали код $2, получили $3"; fails=$((fails + 1)); fi
}

T=$(mktemp -d) && trap 'rm -rf "$T"' EXIT

# 1. Модуль не разбирается — push не уходит.
set -- $(make_repo "$T/a" src/Catalogs/Товары/Ext/ObjectModule.bsl)
cd "$T/a" || exit 1
fake_analyzer "$T/bin-bad" '{"code":"ParseError","message":"ожидается КонецЕсли"}'
PATH="$T/bin-bad:$PATH" run_hook "$1" "$2" >"$T/a.out" 2>&1
check "битый модуль не пускается" 1 $?
grep -q 'ParseError' "$T/a.out" || { say "FAIL хук не показал, что именно нашёл: $(cat "$T/a.out")"; fails=$((fails + 1)); }

# 2. Замечание другого рода — не наше дело, пропускаем.
set -- $(make_repo "$T/b" src/Catalogs/Товары/Ext/ObjectModule.bsl)
cd "$T/b" || exit 1
fake_analyzer "$T/bin-ok" '{"code":"CodeOutOfRegion","message":"вне области"}'
PATH="$T/bin-ok:$PATH" run_hook "$1" "$2" >/dev/null 2>&1
check "прочие диагностики не запирают push" 0 $?

# 3. Анализатора в PATH нет — хук не мешает работать.
set -- $(make_repo "$T/c" src/Catalogs/Товары/Ext/ObjectModule.bsl)
cd "$T/c" || exit 1
PATH="$T/empty:/usr/bin:/bin" run_hook "$1" "$2" >/dev/null 2>&1
check "без анализатора push проходит" 0 $?

# 4. Модулей в правке нет — анализатор не зовётся вовсе.
set -- $(make_repo "$T/d" src/Catalogs/Товары/Ext/form.xml)
cd "$T/d" || exit 1
fake_analyzer "$T/bin-trap" '{"code":"ParseError","message":"не должен был запуститься"}'
PATH="$T/bin-trap:$PATH" run_hook "$1" "$2" >"$T/d.out" 2>&1
check "без .bsl в правке анализатор не зовётся" 0 $?
grep -q 'ParseError' "$T/d.out" && { say "FAIL анализатор всё-таки позвали"; fails=$((fails + 1)); }

[ "$fails" = 0 ] && say "все проверки пройдены" || say "провалено проверок: $fails"
exit $([ "$fails" = 0 ] && echo 0 || echo 1)
