# Тесты модуля scenario. Чистая логика разбора TOML — без gi/GTK.
import pathlib
import unittest

from scenario import TARGET_KINDS, load

_HERE = pathlib.Path(__file__).parent
SCENARIO = str(_HERE.parent / 'scenario.toml')
BROKEN_SCENARIO = str(_HERE / 'fixtures' / 'scenario_broken.toml')
MISSING_FIELD_SCENARIO = str(_HERE / 'fixtures' / 'scenario_missing_field.toml')
MISSING_ACTION_SCENARIO = str(_HERE / 'fixtures' / 'scenario_missing_action.toml')


class LoadTests(unittest.TestCase):
    def test_six_steps_in_order(self):
        steps = load(SCENARIO)
        self.assertEqual([s.number for s in steps], [1, 2, 3, 4, 5, 6])

    def test_texts_are_verbatim(self):
        steps = load(SCENARIO)
        self.assertEqual(steps[1].title, 'Сначала знакомое')
        self.assertIn('заведите одну номенклатуру', steps[1].body)

    def test_no_git_word_anywhere(self):
        # Слово проверяем во всех текстовых полях шага: заголовок, тело,
        # кнопки и готовые вопросы — это тоже текст, который видит человек.
        steps = load(SCENARIO)
        text = ' '.join(
            ' '.join((s.title, s.body, *s.buttons, *s.questions))
            for s in steps
        ).lower()
        self.assertNotIn('git', text)

    def test_unknown_target_kind_is_rejected(self):
        with self.assertRaises(ValueError):
            load(BROKEN_SCENARIO)   # цель kind = "луна"

    def test_missing_required_field_is_rejected(self):
        with self.assertRaises(ValueError):
            load(MISSING_FIELD_SCENARIO)

    def test_target_kinds_constant(self):
        self.assertEqual(TARGET_KINDS, ('none', 'desktop_icon', 'window', 'tray'))

    def test_step_targets_match_brief(self):
        steps = load(SCENARIO)
        self.assertEqual(steps[0].target.kind, 'none')
        self.assertEqual(steps[1].target.kind, 'desktop_icon')
        self.assertEqual(steps[1].target.desktop_file, '1c-start.desktop')
        self.assertEqual(steps[2].target.kind, 'desktop_icon')
        self.assertEqual(steps[2].target.desktop_file, 'dev.zed.Zed.desktop')
        self.assertEqual(steps[3].target.kind, 'window')
        self.assertEqual(steps[3].target.anchor, 'right-bottom')
        self.assertEqual(steps[4].target.kind, 'window')
        self.assertEqual(steps[4].target.anchor, 'right-bottom')
        self.assertEqual(steps[5].target.kind, 'tray')

    def test_wait_for_separates_advance_condition_from_arrow_target(self):
        # Шаги 2 и 3 показывают на значок запуска (он на столе всегда),
        # но ждать обязаны не его — окно 1С/Zed, которое появляется по
        # факту (раздел «Ограничения» спецификации: «шаг закрывается сам,
        # когда окно 1С появилось»). У остальных шагов wait_for не задан —
        # Machine сама падает обратно на target (см. tests.test_steps).
        steps = load(SCENARIO)
        self.assertIsNone(steps[0].wait_for)
        self.assertEqual(steps[1].wait_for.kind, 'window')
        self.assertEqual(steps[1].wait_for.name_pattern, 'Управление нашей фирмой')
        self.assertEqual(steps[2].wait_for.kind, 'window')
        self.assertEqual(steps[2].wait_for.name_pattern, 'Zed')
        self.assertIsNone(steps[3].wait_for)
        self.assertIsNone(steps[4].wait_for)
        self.assertIsNone(steps[5].wait_for)

    def test_advance_on(self):
        steps = load(SCENARIO)
        self.assertEqual([s.advance_on for s in steps], [
            'button', 'target', 'target', 'button', 'button', 'button',
        ])

    def test_step_six_has_only_done_button(self):
        steps = load(SCENARIO)
        self.assertEqual(steps[5].buttons, ('Готово',))

    def test_step_four_questions_are_separate_from_body(self):
        steps = load(SCENARIO)
        self.assertNotIn('Какая номенклатура', steps[3].body)
        self.assertEqual(steps[3].questions, (
            'Какая номенклатура есть в базе',
            'Покажи структуру документа «Заказ покупателя»',
            'Сколько справочников и документов в этой конфигурации',
        ))


class ActionTests(unittest.TestCase):
    # Подпись кнопки -> каноническое действие Machine.press ('next'/'close')
    # и, у двух кнопок, команда, которую нужно запустить. Задача 6: main.py
    # не хранит эту карту в коде, она лежит здесь же, рядом с подписями.
    def test_every_button_has_an_action(self):
        steps = load(SCENARIO)
        for s in steps:
            self.assertEqual(set(s.actions), set(s.buttons))

    def test_close_only_on_step_one(self):
        steps = load(SCENARIO)
        self.assertEqual(steps[0].actions['Закрыть'].action, 'close')
        self.assertEqual(steps[0].actions['Начать'].action, 'next')
        # Крестик/Esc закрывают любой шаг (это их дело, не сценария), но
        # среди кнопок самого сценария 'close' есть только на первом экране.
        for s in steps[1:]:
            self.assertTrue(all(a.action == 'next' for a in s.actions.values()))

    def test_buttons_that_launch_something(self):
        steps = load(SCENARIO)
        self.assertEqual(steps[1].actions['Открыть 1С'].launch, ('1c-stand', 'start1c'))
        self.assertIsNone(steps[1].actions['Дальше'].launch)
        self.assertEqual(steps[2].actions['Закрыть 1С и открыть редактор'].launch, ('zed',))
        self.assertIsNone(steps[2].actions['Дальше'].launch)

    def test_missing_action_is_rejected(self):
        with self.assertRaises(ValueError):
            load(MISSING_ACTION_SCENARIO)


if __name__ == '__main__':
    unittest.main()
