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

# Отметка «онбординг закрыт в этом сеансе» — в $XDG_RUNTIME_DIR (tmpfs,
# живёт до конца сеанса), а не в домашнем каталоге: по решению заказчика
# стенд показывает онбординг при каждом новом входе, а не один раз на
# учётную запись — через три учётки стенда проходит много гостей подряд.
_MARKER_NAME = 'dev1c-onboarding-closed'


def _marker_path() -> pathlib.Path:
    return pathlib.Path(os.environ.get('XDG_RUNTIME_DIR', '/tmp')) / _MARKER_NAME


def already_closed() -> bool:
    return _marker_path().exists()


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
        self._refresh()
        GLib.timeout_add(TICK_MS, self._on_tick)

    def _on_tick(self):
        if self._machine.done:
            return False
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
            _launch(action.launch)
            self._machine.press(action.action)
        self._refresh()

    def _refresh(self):
        step = self._machine.current
        if step is None:
            if self._callout is not None:
                self._callout.hide()
                self._callout = None
            mark_closed()
            Gtk.main_quit()
            return
        target = self._machine.visible_target()
        layout = place(self._world.screen(), target, bubble_size(step))
        if step is self._step and layout == self._layout:
            return  # ничего не изменилось с прошлого тика — не мигаем окном
        self._step, self._layout = step, layout
        if self._callout is not None:
            self._callout.hide()
        self._callout = Callout(step, layout, self._on_button)
        self._callout.show()


def run(scenario_path: str) -> int:
    """Точка входа: 0, если онбординг показан и закрыт (или был закрыт
    раньше в этом сеансе и не показывался вовсе)."""
    if already_closed():
        return 0
    steps = scenario.load(scenario_path)
    App(steps)
    Gtk.main()
    return 0


if __name__ == '__main__':
    default_path = str(pathlib.Path(__file__).parent / 'scenario.toml')
    sys.exit(run(sys.argv[1] if len(sys.argv) > 1 else default_path))
