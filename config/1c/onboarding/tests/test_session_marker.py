# Отметка «закрыт в этом сеансе» обязана умирать вместе с сеансом.
#
# Регрессия со стенда (20.10.2026): отметка была файлом в /tmp, гость под
# dev1 закрыл подсказку, вышел, зашёл снова — и онбординг больше не
# появился, потому что /tmp в контейнере живёт, пока жив контейнер.
# Проверяем именно это: пережить перезапуск X-сервера отметка не должна.
import os
import subprocess
import time
import unittest

import main

DISPLAY = ':96'


def _start_x():
    proc = subprocess.Popen(['Xvfb', DISPLAY, '-screen', '0', '640x480x24', '-ac'],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(50):
        probe = subprocess.run(['xprop', '-root', 'XPROBE'],
                               capture_output=True, text=True,
                               env={**os.environ, 'DISPLAY': DISPLAY})
        if probe.returncode == 0:
            return proc
        time.sleep(0.1)
    proc.terminate()
    raise RuntimeError('Xvfb не поднялся на ' + DISPLAY)


def _stop_x(proc):
    proc.terminate()
    proc.wait(timeout=10)


class SessionMarkerTests(unittest.TestCase):
    def setUp(self):
        self._old_display = os.environ.get('DISPLAY')
        os.environ['DISPLAY'] = DISPLAY
        # Отметка от прошлого прогона не должна утекать в этот.
        main._marker_path().unlink(missing_ok=True)

    def tearDown(self):
        main._marker_path().unlink(missing_ok=True)
        if self._old_display is None:
            os.environ.pop('DISPLAY', None)
        else:
            os.environ['DISPLAY'] = self._old_display

    def test_marker_lives_within_one_session(self):
        x = _start_x()
        try:
            self.assertFalse(main.already_closed())
            main.mark_closed()
            self.assertTrue(main.already_closed())
        finally:
            _stop_x(x)

    def test_marker_does_not_survive_the_session(self):
        x = _start_x()
        try:
            main.mark_closed()
            self.assertTrue(main.already_closed())
        finally:
            _stop_x(x)

        # Новый сеанс на том же номере дисплея — отметки быть не должно.
        x = _start_x()
        try:
            self.assertFalse(main.already_closed())
        finally:
            _stop_x(x)

    def test_absent_display_is_not_a_closed_session(self):
        old = os.environ.pop('DISPLAY', None)
        try:
            self.assertFalse(main.already_closed())
            main.mark_closed()  # падать нельзя
            # Сеанс опознать нельзя, отметка есть — всё равно показываем:
            # лишний показ лучше, чем молчание (ruling 21.10).
            self.assertFalse(main.already_closed())
        finally:
            if old is not None:
                os.environ['DISPLAY'] = old


if __name__ == '__main__':
    unittest.main()
