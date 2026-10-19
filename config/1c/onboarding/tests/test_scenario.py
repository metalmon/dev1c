# Тесты модуля scenario. Чистая логика разбора TOML — без gi/GTK.
import pathlib
import unittest

from scenario import TARGET_KINDS, load

_HERE = pathlib.Path(__file__).parent
SCENARIO = str(_HERE.parent / 'scenario.toml')
BROKEN_SCENARIO = str(_HERE / 'fixtures' / 'scenario_broken.toml')
MISSING_FIELD_SCENARIO = str(_HERE / 'fixtures' / 'scenario_missing_field.toml')


class LoadTests(unittest.TestCase):
    def test_six_steps_in_order(self):
        steps = load(SCENARIO)
        self.assertEqual([s.number for s in steps], [1, 2, 3, 4, 5, 6])

    def test_texts_are_verbatim(self):
        steps = load(SCENARIO)
        self.assertEqual(steps[1].title, 'Сначала знакомое')
        self.assertIn('заведите одну номенклатуру', steps[1].body)

    def test_no_git_word_anywhere(self):
        text = ' '.join(s.title + ' ' + s.body for s in load(SCENARIO)).lower()
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


if __name__ == '__main__':
    unittest.main()
