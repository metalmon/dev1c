# Главная программа онбординга: связывает сценарий, машину шагов, живой
# мир и окно выноски. Запускается при входе в сессию (onboarding-autostart.
# desktop) и из меню приложений (onboarding.desktop) — оба зовут
# «1c-stand onboarding», а не python3 напрямую: путь к интерпретатору и
# к сценарию остаётся в одном месте, в stand.sh.
import os
import pathlib
import subprocess
import sys

import gi

gi.require_version('Gtk', '3.0')
gi.require_version('Gdk', '3.0')
from gi.repository import Gdk, GLib, Gtk

import scenario
from callout import Callout
from placement import bubble_size, place
from steps import Machine
from world import World

TICK_MS = 1000  # как часто проверять, не появилась ли цель текущего шага

# Сколько ждать перед проверкой, закрылась ли 1С сама после вежливой
# просьбы (close_window, шаг 3): окну нужно время среагировать на
# _NET_CLOSE_WINDOW (возможно, спросить «сохранить изменения?»).
CLOSE_CHECK_DELAY_S = 4

# Тот же текст для обоих поводов его показать: окно всё ещё есть через
# CLOSE_CHECK_DELAY_S, или wmctrl в образе вовсе нет (FileNotFoundError —
# тот самый образ, который ещё не пересобрали с новым пакетом). Человеку
# не важно, почему не получилось закрыть — важно, что не получилось.
_WINDOW_NOT_CLOSED_WARNING = ('Похоже, окно 1С не закрылось само — закройте его '
                              'вручную, иначе агент не сможет зайти в базу.')

# Отметка «онбординг закрыт в этом сеансе» — свойство на корневом окне
# X-сервера. По решению заказчика стенд показывает онбординг при каждом
# новом входе, а не один раз на учётную запись: через три учётки стенда
# проходит много гостей подряд.
#
# Раньше отметка была файлом в $XDG_RUNTIME_DIR с запасным путём /tmp.
# В сеансе xrdp на стенде XDG_RUNTIME_DIR не выставлен (ни systemd, ни
# elogind в образе нет), поэтому в деле всегда работал запасной путь —
# а /tmp в контейнере живёт, пока жив контейнер, а не сеанс. Закрыв
# подсказку один раз, гость закрывал её навсегда: следующий вход под той
# же учёткой уже молчал. Найдено на стенде под dev1 (20.10.2026).
#
# Сеанс опознаётся по сокету своего X-сервера: /tmp/.X11-unix/X<N>
# создаётся заново при каждом старте сервера, поэтому его время —
# это время начала сеанса. Отметка остаётся файлом, но считается
# действительной, только если сделана позже начала текущего сеанса.
# Свойство на корневом окне было бы прямее, но xprop в этом образе
# молча не пишет (-set возвращает 0 и ничего не сохраняет), а
# Gdk.property_change в этой сборке gi не представлен — проверено.
_MARKER_NAME = 'dev1c-onboarding-closed'


def _marker_path() -> pathlib.Path:
    return pathlib.Path(os.environ.get('XDG_RUNTIME_DIR', '/tmp')) / _MARKER_NAME


def _session_started_at() -> float | None:
    """Время старта X-сервера текущего дисплея, None — если не узнать."""
    display = os.environ.get('DISPLAY', '')
    number = display.partition(':')[2].partition('.')[0]
    if not number.isdigit():
        return None
    try:
        return pathlib.Path('/tmp/.X11-unix/X' + number).stat().st_mtime
    except OSError:
        return None


def already_closed() -> bool:
    """Закрывал ли гость подсказку в этом сеансе.

    Отметка, оставшаяся от прошлого входа, не считается: без этой
    проверки /tmp в контейнере превращал «закрыл в этом сеансе» в
    «закрыл навсегда».
    """
    marker = _marker_path()
    try:
        closed_at = marker.stat().st_mtime
    except OSError:
        return False
    started_at = _session_started_at()
    if started_at is None:
        # Сеанс опознать не удалось. Лишний показ подсказки — мелкая
        # неловкость, молчание — та самая жалоба, с которой всё
        # началось: гость не увидел онбординг вовсе. Выбираем показ.
        return False
    return closed_at >= started_at


def mark_closed() -> None:
    _marker_path().touch()


def copy_question(text: str) -> bool:
    """Кладёт text в буфер обмена (шаг 4: готовые вопросы агенту).

    False вместо исключения, если дисплея нет или он недоступен.

    Проверка DISPLAY — не только защита до вызова Gdk (get_default() и
    без неё тихо вернул бы None, без исключения): once-per-process GTK
    держит дисплей, к которому подключился первым, и голый
    get_default() не заметил бы, что DISPLAY потом исчез — как раз
    случай этого теста. Дальше try/except на случай ошибки буфера."""
    if not os.environ.get('DISPLAY'):
        return False
    try:
        display = Gdk.Display.get_default()
        if display is None:
            return False
        clipboard = Gtk.Clipboard.get_default(display)
        clipboard.set_text(text, -1)
        clipboard.store()
        return True
    except Exception:
        return False


def _launch(command) -> None:
    """Запускает command (кортеж строк) в фоне, не дожидаясь — так кнопки
    «Открыть 1С» и «Закрыть 1С и открыть редактор» не блокируют выноску."""
    if command:
        subprocess.Popen(list(command))


class App:
    """Один экран онбординга поверх живого рабочего стола."""

    def __init__(self, steps):
        self._world = World()
        self._machine = Machine(steps, self._world)
        self._callout = None
        self._step = None
        self._layout = None
        # Сообщение, которое дописывается к телу текущего шага (не текст
        # сценария — это main.py говорит от своего имени, что 1С не
        # закрылась сама). _warning_pattern — какое окно отслеживается,
        # чтобы снять предупреждение, когда оно и правда закрылось (см.
        # _on_tick) — человек мог закрыть 1С вручную уже на шаге 4 или
        # позже, после того как разовая проверка решила, что не вышло.
        self._warning = None
        self._warning_pattern = None
        self._shown_warning = None
        self._refresh()
        GLib.timeout_add(TICK_MS, self._on_tick)

    def _on_tick(self):
        if self._machine.done:
            return False
        if self._warning_pattern is not None and not self._world.window_present(self._warning_pattern):
            self._clear_warning()
        self._machine.tick()
        self._refresh()
        return not self._machine.done

    def _on_button(self, label):
        step = self._machine.current
        if step is None:
            return
        if label == 'close':
            # Крестик и Esc самой выноски: callout.py отдаёт их напрямую
            # этой строкой, мимо карты действий сценария (там 'close'
            # как подпись кнопки есть только на шаге 1, под «Закрыть»).
            self._machine.press('close')
        else:
            action = step.actions[label]
            # launch и таймер — раньше попытки close_window и несмотря
            # на её исход: если wmctrl в образе нет (FileNotFoundError —
            # образ ещё не пересобран с новым пакетом), редактор всё
            # равно должен открыться и предупреждение всё равно должно
            # появиться, а не провалить весь обработчик молча.
            _launch(action.launch)
            if action.close_window:
                GLib.timeout_add_seconds(CLOSE_CHECK_DELAY_S, self._check_close_window, action.close_window)
                try:
                    self._world.close_window(action.close_window)
                except FileNotFoundError:
                    self._set_warning(action.close_window, _WINDOW_NOT_CLOSED_WARNING)
            # 'stay': кнопка не двигает Machine сама — шаг и так уходит
            # дальше по wait_for, когда его условие выполнится. Силой —
            # только 'next'/'close'.
            if action.action != 'stay':
                self._machine.press(action.action)
        self._refresh()

    def _check_close_window(self, pattern):
        """Разовая проверка через CLOSE_CHECK_DELAY_S после close_window:
        окно всё ещё есть — говорим об этом в выноске, а не делаем вид,
        что всё получилось. Дальше предупреждение снимает _on_tick, как
        только окно и правда исчезнет — не обязательно от этой самой
        проверки."""
        if self._world.window_present(pattern):
            self._set_warning(pattern, _WINDOW_NOT_CLOSED_WARNING)
        return False  # не повторять: дальше следит _on_tick

    def _set_warning(self, pattern, text):
        self._warning = text
        self._warning_pattern = pattern
        self._refresh(force=True)

    def _clear_warning(self):
        self._warning = None
        self._warning_pattern = None
        self._refresh(force=True)

    def _refresh(self, force=False):
        step = self._machine.current
        if step is None:
            if self._callout is not None:
                self._callout.destroy()
                self._callout = None
            mark_closed()
            Gtk.main_quit()
            return
        display_step = step if self._warning is None else step._replace(body=step.body + '\n\n' + self._warning)
        target = self._machine.visible_target()
        layout = place(self._world.screen(), target, bubble_size(display_step))
        if not force and step is self._step and layout == self._layout and self._warning == self._shown_warning:
            return  # ничего не изменилось с прошлого тика — не мигаем окном
        self._step, self._layout, self._shown_warning = step, layout, self._warning
        if self._callout is not None:
            self._callout.destroy()
        self._callout = Callout(display_step, layout, self._on_button, copy_question,
                                total=self._machine.total)
        self._callout.show()


def run(scenario_path: str, gate: bool = False) -> int:
    """Точка входа: 0, если онбординг показан и закрыт.

    gate=True — уважать отметку «закрыт в этом сеансе» (автозапуск
    сессии: onboarding-autostart.desktop зовёт `1c-stand onboarding
    --autostart»). gate=False (по умолчанию) — показать всегда: пункт
    меню «Знакомство со стендом» (onboarding.desktop, без флага) обязан
    работать и после первого закрытия — иначе после первого же раза он
    молчал бы до конца сеанса (финальная волна)."""
    if gate and already_closed():
        return 0
    steps = scenario.load(scenario_path)
    App(steps)
    Gtk.main()
    return 0


if __name__ == '__main__':
    args = sys.argv[1:]
    gate = '--autostart' in args
    args = [a for a in args if a != '--autostart']
    default_path = str(pathlib.Path(__file__).parent / 'scenario.toml')
    sys.exit(run(args[0] if args else default_path, gate=gate))
