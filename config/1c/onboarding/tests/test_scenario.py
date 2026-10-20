# Тесты модуля scenario. Чистая логика разбора TOML — без gi/GTK.
import pathlib
import unittest

from scenario import TARGET_KINDS, load

_HERE = pathlib.Path(__file__).parent
SCENARIO = str(_HERE.parent / 'scenario.toml')
BROKEN_SCENARIO = str(_HERE / 'fixtures' / 'scenario_broken.toml')
MISSING_FIELD_SCENARIO = str(_HERE / 'fixtures' / 'scenario_missing_field.toml')
MISSING_ACTION_SCENARIO = str(_HERE / 'fixtures' / 'scenario_missing_action.toml')
BAD_ANCHOR_SCENARIO = str(_HERE / 'fixtures' / 'scenario_bad_anchor.toml')


class LoadTests(unittest.TestCase):
    def test_two_acts_numbered_through(self):
        steps = load(SCENARIO)
        self.assertEqual([s.number for s in steps], list(range(1, 15)))
        # Нумерация сквозная, а принадлежность к акту — отдельное поле:
        # человек видит «Шаг 1 из 7» в обоих актах (Machine.position).
        self.assertEqual([s.number for s in steps if s.act == 1], [1, 2, 3, 4, 5, 6, 7])
        self.assertEqual([s.number for s in steps if s.act == 2], [8, 9, 10, 11, 12, 13, 14])

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

    def test_bad_anchor_is_rejected_at_load_time(self):
        # Финальная волна: опечатка в anchor должна падать здесь, на
        # load(), а не когда-нибудь потом внутри targets.anchor_in на
        # живом стенде.
        with self.assertRaises(ValueError):
            load(BAD_ANCHOR_SCENARIO)   # anchor = "rihgt-bottom"

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
        self.assertEqual(steps[6].target.kind, 'desktop_icon')
        self.assertEqual(steps[6].target.desktop_file, 'gitea.desktop')

    def test_no_step_carries_a_wait_for(self):
        # Шаги 2 и 3 (ruling второй и этой волны) wait_for не несут
        # вовсе: появление окна 1С/Zed значит «человек начал», не
        # «закончил» — вводить позицию (шаг 2) или нажимать кнопку,
        # которая закрывает 1С (шаг 3, пока Zed не открылся сам и шаг не
        # проскочил мимо неё). Остальные шаги никогда wait_for не несли.
        # Machine в его отсутствие падает обратно на target (см.
        # tests.test_steps) — поле у Step осталось, просто сценарию
        # больше негде им пользоваться.
        steps = load(SCENARIO)
        self.assertTrue(all(s.wait_for is None for s in steps))

    def test_advance_on(self):
        steps = load(SCENARIO)
        # Каждый шаг ждёт кнопку, без исключений — проверяем именно это,
        # а не длину списка: число шагов сценария меняется, правило нет.
        self.assertEqual({s.advance_on for s in steps}, {'button'})

    def test_last_step_has_only_done_button(self):
        steps = load(SCENARIO)
        self.assertEqual(steps[-1].buttons, ('Готово',))

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

    def test_close_only_where_the_person_is_offered_a_way_out(self):
        steps = load(SCENARIO)
        self.assertEqual(steps[0].actions['Закрыть'].action, 'close')
        self.assertEqual(steps[0].actions['Начать'].action, 'next')
        # Крестик/Esc закрывают любой шаг (это их дело, не сценария).
        # Среди кнопок сценария 'close' стоит ровно там, где человеку
        # предлагают выход: первый экран, конец первого акта («Готово»
        # вместо второго акта), развилка второго акта и его конец.
        closing = {s.number for s in steps
                   if any(a.action == 'close' for a in s.actions.values())}
        self.assertEqual(closing, {1, 7, 8, 14})

    def test_step_three_button_advances_and_closes_1c(self):
        # Кнопка сама двигает шаг («next» — wait_for у шага 3 больше нет,
        # ждать нечего) и вежливо просит 1С закрыться (close_window).
        steps = load(SCENARIO)
        action = steps[2].actions['Закрыть 1С и открыть редактор']
        self.assertEqual(action.action, 'next')
        self.assertEqual(action.close_window, 'Управление нашей фирмой')
        self.assertIsNone(steps[2].actions['Дальше'].close_window)

    def test_buttons_that_launch_something(self):
        steps = load(SCENARIO)
        self.assertEqual(steps[1].actions['Открыть 1С'].launch, ('1c-stand', 'start1c'))
        self.assertIsNone(steps[1].actions['Дальше'].launch)
        self.assertEqual(steps[2].actions['Закрыть 1С и открыть редактор'].launch, ('zed',))
        self.assertIsNone(steps[2].actions['Дальше'].launch)

    def test_missing_action_is_rejected(self):
        with self.assertRaises(ValueError):
            load(MISSING_ACTION_SCENARIO)

    def test_actions_are_not_a_plain_mutable_dict(self):
        # Финальная волна: Step.actions по умолчанию был {} — один
        # разделяемый объект на все шаги без своих actions. Попытка
        # записи в него должна падать, а не тихо портить чужой default.
        steps = load(SCENARIO)
        with self.assertRaises(TypeError):
            steps[0].actions['новая'] = None


if __name__ == '__main__':
    unittest.main()
