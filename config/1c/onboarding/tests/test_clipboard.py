# Тест на буфер обмена: copy_question не должен падать там, где дисплея
# нет (CI, сборка образа) — туда ONBOARDING_TEST_DISPLAY/DISPLAY не
# указывают вовсе.
import os
import unittest

from main import copy_question


class CopyQuestionTests(unittest.TestCase):
    def test_copy_question_survives_absent_display(self):
        # DISPLAY пуст: буфера нет, но падать нельзя
        old = os.environ.pop('DISPLAY', None)
        try:
            self.assertFalse(copy_question('Какая номенклатура есть в базе'))
        finally:
            if old is not None:
                os.environ['DISPLAY'] = old


if __name__ == '__main__':
    unittest.main()
